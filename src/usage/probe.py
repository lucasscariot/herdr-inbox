# Usage probe for Herdr Inbox. Runs on each machine with Python 3.9+, from
# source, so nothing needs installing there. Reads one JSON argument:
#   {"state_dir": "/path" | null, "codex": "codex" | null, "claude": bool,
#    "statusline": "<the status line wrapper's source>"}
# and prints {"anthropic": {...}, "openai": {...}, "statusline": "/path"}.
#
# Every provider entry is {"usage": {...}} or {"error": "why"}. A usage has
# windows [{"label", "used_percent", "resets_at"}], "plan", "reached",
# "resets_available" and "source". Nothing here reads or sends a credential:
# Codex answers its own app server, Claude Code hands its numbers to the
# status line wrapper, which caches them under the state directory.
import glob, json, os, pathlib, subprocess, sys, tempfile, time

settings = json.loads(sys.argv[1])
now = int(time.time())
state_dir = settings.get("state_dir")
if not state_dir:
    base = os.environ.get("XDG_STATE_HOME") or str(pathlib.Path.home() / ".local/state")
    state_dir = str(pathlib.Path(base) / "herdr-inbox")
usage_dir = pathlib.Path(state_dir) / "usage"


def window_label(minutes):
    if minutes is None:
        return "window"
    minutes = int(minutes)
    if minutes == 10080:
        return "week"
    if minutes % 1440 == 0:
        return "%dd" % (minutes // 1440)
    if minutes % 60 == 0:
        return "%dh" % (minutes // 60)
    return "%dm" % minutes


def install_statusline():
    """Writes the status line wrapper once; returns its path or None."""
    source = settings.get("statusline")
    if not source:
        return None
    path = usage_dir / "statusline.py"
    try:
        usage_dir.mkdir(parents=True, exist_ok=True)
        os.chmod(usage_dir, 0o700)
        if not path.is_file() or path.read_text() != source:
            fd, tmp = tempfile.mkstemp(prefix=".statusline-", dir=str(usage_dir))
            with os.fdopen(fd, "w") as handle:
                handle.write(source)
            os.chmod(tmp, 0o700)
            os.replace(tmp, path)
        return str(path)
    except OSError:
        return None


def anthropic():
    """Claude Code's numbers, as the status line wrapper last saw them."""
    cache = usage_dir / "anthropic.json"
    try:
        saved = json.loads(cache.read_text())
    except (OSError, ValueError):
        return {"error": "no Claude thread has reported yet"}
    limits = saved.get("rate_limits") if isinstance(saved, dict) else None
    if not isinstance(limits, dict):
        return {"error": "no Claude thread has reported yet"}
    windows = []
    for key, label in (("five_hour", "5h"), ("seven_day", "week")):
        window = limits.get(key)
        if not isinstance(window, dict) or window.get("used_percentage") is None:
            continue
        try:
            used = int(round(float(window["used_percentage"])))
        except (TypeError, ValueError):
            continue
        resets = window.get("resets_at")
        windows.append({"label": label, "used_percent": max(0, used), "resets_at": int(resets) if isinstance(resets, (int, float)) else None})
    if not windows:
        return {"error": "Claude Code reported no usage windows (an API key login has none)"}
    read_at = saved.get("read_at")
    return {"usage": {
        "windows": windows,
        "plan": None,
        "reached": any(w["used_percent"] >= 100 for w in windows),
        "refused": False,
        "allowed": None,
        "resets_available": 0,
        "read_at": int(read_at) if isinstance(read_at, (int, float)) else now,
        "source": "statusline",
    }}


def codex_windows(snapshot, camel):
    windows = []
    for key in ("primary", "secondary"):
        window = snapshot.get(key)
        if not isinstance(window, dict):
            continue
        used = window.get("usedPercent" if camel else "used_percent")
        minutes = window.get("windowDurationMins" if camel else "window_minutes")
        resets = window.get("resetsAt" if camel else "resets_at")
        if used is None:
            continue
        try:
            used = int(round(float(used)))
        except (TypeError, ValueError):
            continue
        windows.append({"label": window_label(minutes), "used_percent": max(0, used), "resets_at": int(resets) if isinstance(resets, (int, float)) else None})
    return windows


def codex_usage(snapshot, camel, resets_available, read_at, source, allowed=None):
    """`allowed` is the app server's own verdict (ordinaryUsageAllowed): when
    present it decides, whatever the percentages say."""
    windows = codex_windows(snapshot, camel)
    if not windows:
        return {"error": "Codex reported no usage windows"}
    reached_key = "rateLimitReachedType" if camel else "rate_limit_reached_type"
    spend_key = "spendControlReached" if camel else "spend_control_reached"
    plan = snapshot.get("planType" if camel else "plan_type")
    # The provider's own verdict, kept apart from a spent window: a window's
    # reset clears the latter, only the next reading clears the former.
    if allowed is None:
        refused = bool(snapshot.get(reached_key)) or bool(snapshot.get(spend_key))
        reached = refused or any(w["used_percent"] >= 100 for w in windows)
    else:
        refused = reached = not allowed
    return {"usage": {
        "windows": windows,
        "plan": plan if isinstance(plan, str) else None,
        "reached": reached,
        "refused": refused,
        "allowed": allowed,
        "resets_available": resets_available,
        "read_at": read_at,
        "source": source,
    }}


def codex_app_server(executable):
    """Asks Codex's own app server, the client protocol it documents."""
    env = {key: value for key, value in os.environ.items() if not key.startswith("HERDR_")}
    requests = [
        {"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": {"clientInfo": {"name": "herdr_inbox", "title": "Herdr Inbox", "version": "1"}}},
        {"jsonrpc": "2.0", "method": "initialized", "params": {}},
        {"jsonrpc": "2.0", "id": 1, "method": "account/rateLimits/read", "params": {}},
    ]
    process = subprocess.Popen([executable, "app-server"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env, start_new_session=True, text=True)
    # A server that never answers is killed, which ends the read below.
    import threading
    timer = threading.Timer(10, lambda: process.kill())
    timer.start()
    try:
        process.stdin.write("".join(json.dumps(request) + "\n" for request in requests))
        process.stdin.flush()
        for line in process.stdout:
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if message.get("id") == 1:
                if "error" in message:
                    raise RuntimeError(str(message["error"].get("message", message["error"])))
                result = message.get("result") or {}
                snapshot = result.get("rateLimits") or {}
                credits = result.get("rateLimitResetCredits") or {}
                available = credits.get("availableCount") if isinstance(credits, dict) else 0
                allowed = result.get("ordinaryUsageAllowed")
                return codex_usage(snapshot, True, int(available or 0), now, "app-server", allowed if isinstance(allowed, bool) else None)
        raise RuntimeError("the Codex app server did not answer in time")
    finally:
        timer.cancel()
        try:
            process.kill()
        except OSError:
            pass
        process.wait()


def codex_sessions():
    """The newest rate limits Codex wrote to its own session log, within a week."""
    home = pathlib.Path(os.environ.get("CODEX_HOME", str(pathlib.Path.home() / ".codex"))).expanduser()
    files = glob.glob(str(home / "sessions" / "**" / "*.jsonl"), recursive=True)
    files = [f for f in files if now - os.path.getmtime(f) < 7 * 86400]
    files.sort(key=os.path.getmtime, reverse=True)
    for path in files[:20]:
        try:
            lines = pathlib.Path(path).read_text(errors="ignore").splitlines()
        except OSError:
            continue
        for line in reversed(lines):
            if '"rate_limits"' not in line:
                continue
            try:
                event = json.loads(line)
            except ValueError:
                continue
            payload = event.get("payload") if isinstance(event, dict) else None
            snapshot = payload.get("rate_limits") if isinstance(payload, dict) else None
            if not isinstance(snapshot, dict):
                continue
            stamp = event.get("timestamp")
            read_at = int(os.path.getmtime(path))
            if isinstance(stamp, str):
                try:
                    from datetime import datetime, timezone
                    read_at = int(datetime.fromisoformat(stamp.replace("Z", "+00:00")).astimezone(timezone.utc).timestamp())
                except ValueError:
                    pass
            return codex_usage(snapshot, False, 0, read_at, "session")
    return {"error": "no Codex session has reported usage"}


def openai(executable):
    try:
        return codex_app_server(executable)
    except (OSError, RuntimeError, ValueError) as error:
        fallback = codex_sessions()
        if "usage" in fallback:
            return fallback
        return {"error": "Codex app server: %s" % error}


report = {"statusline": install_statusline()}
if settings.get("claude"):
    report["anthropic"] = anthropic()
if settings.get("codex"):
    report["openai"] = openai(settings["codex"])
print(json.dumps(report))
