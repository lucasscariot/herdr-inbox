# Claude Code status line wrapper for Herdr Inbox. Claude Code runs it with
# the documented status line JSON on stdin; the rate limits in it are saved
# privately to the cache file given as the first argument, then the user's
# own status line command, if any, runs on the same input so nothing they
# configured goes missing. Never fails loudly: a status line that errors
# would show up in every Claude thread.
import json, os, pathlib, subprocess, sys, tempfile, time

raw = sys.stdin.buffer.read()
cache = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else None

try:
    data = json.loads(raw.decode("utf-8", "replace"))
except ValueError:
    data = {}
limits = data.get("rate_limits") if isinstance(data, dict) else None
if cache and isinstance(limits, dict):
    try:
        cache.parent.mkdir(parents=True, exist_ok=True)
        fd, tmp = tempfile.mkstemp(prefix=".anthropic-", dir=str(cache.parent))
        with os.fdopen(fd, "w") as handle:
            json.dump({"rate_limits": limits, "read_at": int(time.time())}, handle)
        os.chmod(tmp, 0o600)
        os.replace(tmp, cache)
    except OSError:
        pass

config_dir = os.environ.get("CLAUDE_CONFIG_DIR") or str(pathlib.Path.home() / ".claude")
command = None
try:
    settings = json.loads((pathlib.Path(config_dir) / "settings.json").read_text())
    status = settings.get("statusLine") if isinstance(settings, dict) else None
    if isinstance(status, dict) and status.get("type", "command") == "command":
        command = status.get("command")
except (OSError, ValueError):
    pass
own = os.path.basename(sys.argv[0]) if sys.argv else ""
if isinstance(command, str) and command.strip() and own not in command:
    try:
        result = subprocess.run(["sh", "-c", command], input=raw, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=5, check=False)
        sys.stdout.buffer.write(result.stdout)
        sys.stdout.flush()
    except (OSError, subprocess.SubprocessError):
        pass
