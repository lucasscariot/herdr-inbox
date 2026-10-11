# Claude Code status line wrapper for Herdr Inbox. Claude Code runs it with
# the documented status line JSON on stdin; the rate limits in it are saved
# privately to the cache file given as the first argument, then the user's
# own status line command, if any, runs on the same input so nothing they
# configured goes missing. Never fails loudly: a status line that errors
# would show up in every Claude thread.
import json, os, pathlib, subprocess, sys, tempfile, time

# A user status line that runs this wrapper would recurse: the inner run
# does nothing.
if os.environ.get("HERDR_INBOX_STATUSLINE"):
    sys.exit(0)
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

# The user's own status line, in Claude Code's order of precedence below the
# command line: the project's local settings, the project's settings, then
# the user's. `--settings` hides them all, so the first one found is chained.
config_dir = os.environ.get("CLAUDE_CONFIG_DIR") or str(pathlib.Path.home() / ".claude")
candidates = []
workspace = data.get("workspace") if isinstance(data, dict) else None
project_dir = workspace.get("project_dir") if isinstance(workspace, dict) else None
for base in (project_dir, data.get("cwd") if isinstance(data, dict) else None):
    if isinstance(base, str) and base:
        candidates += [pathlib.Path(base) / ".claude/settings.local.json", pathlib.Path(base) / ".claude/settings.json"]
candidates.append(pathlib.Path(config_dir) / "settings.json")
command = None
for path in candidates:
    try:
        settings = json.loads(path.read_text())
    except (OSError, ValueError):
        continue
    status = settings.get("statusLine") if isinstance(settings, dict) else None
    if isinstance(status, dict) and status.get("type", "command") == "command" and isinstance(status.get("command"), str):
        command = status["command"]
        break
own = {os.path.abspath(sys.argv[0]), os.path.realpath(sys.argv[0])} if sys.argv and sys.argv[0] else set()
if isinstance(command, str) and command.strip() and not any(path in command for path in own):
    try:
        env = dict(os.environ, HERDR_INBOX_STATUSLINE="1")
        result = subprocess.run(["sh", "-c", command], input=raw, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, timeout=5, check=False, env=env)
        sys.stdout.buffer.write(result.stdout)
        sys.stdout.flush()
    except (OSError, subprocess.SubprocessError):
        pass
