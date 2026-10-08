"""Herdr transport. Every call keeps an explicit machine and session target."""

import json
import hashlib
import os
import socket
import shlex
import subprocess
import uuid
from dataclasses import dataclass
from pathlib import Path


class HerdrError(RuntimeError):
    pass


@dataclass(frozen=True)
class Machine:
    id: str
    label: str
    target: str = ""
    session: str = "default"

    @property
    def local(self):
        return self.id == "local"


def local_machine():
    address = Path(os.environ.get("HERDR_SOCKET_PATH", ""))
    session = address.parent.name if address.parent.parent.name == "sessions" else "default"
    return Machine("local", "Local", session=session)


def run_json(argv, timeout=25, env=None, allow_empty=False, cwd=None):
    try:
        process = subprocess.run(argv, stdin=subprocess.DEVNULL, start_new_session=True, capture_output=True, text=True, timeout=timeout, env=env, cwd=cwd)
    except subprocess.TimeoutExpired:
        raise HerdrError("Command timed out. Check the machine before retrying a launch.") from None
    except OSError as error:
        raise HerdrError(str(error)) from error
    output = process.stdout.strip() if process.returncode == 0 else process.stderr.strip()
    if process.returncode and process.stdout.strip():
        try:
            json.loads(process.stdout)
            output = process.stdout.strip()
        except json.JSONDecodeError:
            pass
    if process.returncode == 0 and not output and allow_empty:
        return {}
    try:
        response = json.loads(output)
    except json.JSONDecodeError:
        raise HerdrError(output[-1500:] or "Herdr returned an empty response") from None
    if process.returncode or isinstance(response, dict) and "error" in response:
        detail = response.get("error", response) if isinstance(response, dict) else response
        raise HerdrError(detail.get("message", str(detail)) if isinstance(detail, dict) else str(detail))
    return response.get("result", response) if isinstance(response, dict) else response


def control_dir():
    """Short-lived SSH control sockets, so every call after the first reuses one authenticated session."""
    directory = Path(os.environ.get("TMPDIR", "/tmp")) / ("herdr-inbox-" + str(os.getuid()))
    try:
        directory.mkdir(mode=0o700, exist_ok=True)
        os.chmod(directory, 0o700)
    except OSError:
        return None
    return directory


def host_command(machine, argv, cwd=None):
    if machine.local:
        return list(argv)
    command = 'PATH="$HOME/.local/share/mise/shims:$HOME/.local/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"; export PATH; '
    if cwd:
        command += "cd " + shlex.quote(cwd) + " && "
    command += shlex.join(argv)
    options = ["-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes", "-o", "ConnectTimeout=8", "-o", "ServerAliveInterval=15", "-o", "ServerAliveCountMax=2"]
    sockets = control_dir()
    if sockets is not None:
        options += ["-o", "ControlMaster=auto", "-o", "ControlPath=" + str(sockets / "%C"), "-o", "ControlPersist=600"]
    return ["ssh"] + options + ["--", machine.target, "sh -lc " + shlex.quote(command)]


class Herdr:
    def __init__(self, binary=None):
        self.binary = binary or os.environ.get("HERDR_BIN_PATH", "herdr")

    def call(self, machine, *args, timeout=25):
        argv = [self.binary]
        if not machine.local:
            argv += ["--machine", machine.id]
        argv += list(args)
        return run_json(argv, timeout=timeout, allow_empty=tuple(args[:2]) == ("pane", "report-metadata"))

    def machines(self):
        profiles = run_json([self.binary, "machine", "list", "--json"])
        return [local_machine()] + [Machine(p["id"], p["label"], p["target"], p.get("session", "default")) for p in profiles if p.get("enabled", True)]

    def raw(self, method, params, timeout=15):
        """Use the local public JSON socket for methods without a CLI wrapper, or when speed matters."""
        address = os.environ.get("HERDR_SOCKET_PATH")
        if not address:
            raise HerdrError("Open this action inside Herdr so it has a session socket.")
        request = {"id": "inbox:" + uuid.uuid4().hex, "method": method, "params": params}
        try:
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
                connection.settimeout(timeout)
                connection.connect(address)
                connection.sendall((json.dumps(request) + "\n").encode())
                response = json.loads(connection.makefile("rb").readline())
        except (OSError, ValueError) as error:
            raise HerdrError(str(error)) from error
        if "error" in response:
            raise HerdrError(response["error"].get("message", str(response["error"])))
        return response.get("result", response)

    def open(self, view):
        from .store import read_json, state_dir, write_json

        entrypoint = "launcher" if view == "new" else "inbox"
        session_key = hashlib.sha256(os.environ.get("HERDR_SOCKET_PATH", "").encode()).hexdigest()[:16]
        locator = state_dir() / "composers" / (session_key + "-" + entrypoint + ".json")
        saved = read_json(locator, {})
        if saved:
            panes = self.raw("pane.list", {})["panes"]
            if any(p["pane_id"] == saved["pane_id"] and p.get("tokens", {}).get("inbox_composer") == saved["nonce"] for p in panes):
                return self.raw("plugin.pane.focus", {"pane_id": saved["pane_id"]})
        result = self.raw("plugin.pane.open", {"plugin_id": "lucasscariot.herdr-inbox", "entrypoint": entrypoint, "placement": "tab", "focus": True})
        pane_id = result["plugin_pane"]["pane"]["pane_id"]
        nonce = uuid.uuid4().hex
        self.call(local_machine(), "pane", "report-metadata", pane_id, "--source", "plugin:lucasscariot.herdr-inbox", "--token", "inbox_composer=" + nonce)
        write_json(locator, {"pane_id": pane_id, "nonce": nonce})
        return result

    def agents(self, machine):
        # The local socket answers in a millisecond; the CLI costs a process per poll.
        if machine.local and os.environ.get("HERDR_SOCKET_PATH"):
            return self.raw("agent.list", {}, timeout=10)["agents"]
        return self.call(machine, "agent", "list")["agents"]

    def focus(self, machine, pane_id):
        return self.call(machine, "agent", "focus", pane_id)

    def create_workspace(self, machine, cwd, label):
        return self.call(machine, "workspace", "create", "--cwd", cwd, "--label", label, "--no-focus")

    def create_worktree(self, machine, repo, branch, label, base=""):
        args = ["worktree", "create", "--cwd", repo, "--branch", branch, "--label", label, "--no-focus"]
        if base:
            args += ["--base", base]
        return self.call(machine, *args, timeout=60)

    def prompt(self, machine, pane_id, text, timeout_ms=15000):
        return self.call(machine, "agent", "prompt", pane_id, text, "--wait", "--until", "working", "--until", "blocked", "--timeout", str(timeout_ms), timeout=timeout_ms // 1000 + 10)
