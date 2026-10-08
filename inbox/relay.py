"""Stream a Herdr server's agent state as JSON lines, without polling.

This file is self-contained on purpose: the plugin runs it in-process for the
local server and ships its source to saved machines with ``python3 -c``, so the
same code watches every host. It needs Python 3.9 and the standard library.

Messages emitted, one JSON object per line:

- ``{"type": "hello", "socket": path}`` once connected and subscribed.
- ``{"type": "snapshot", "agents": [...]}`` the full agent list, on connect, on
  pane lifecycle changes, and on a periodic reconcile.
- ``{"type": "agent", "agent": {...}}`` one agent whose status changed.
- ``{"type": "gone", "pane_id": id}`` an agent pane that closed.
- ``{"type": "ping"}`` heartbeat so the reader can tell a stalled link.
- ``{"type": "error", "message": text}`` before exiting on failure.

Commands accepted on stdin, one per line: ``snapshot``.
"""

import json
import os
import selectors
import socket
import sys
import time
import uuid

LIFECYCLE = ("pane.created", "pane.closed", "pane.exited", "pane.agent_detected", "pane.updated", "workspace.closed")
RECONCILE_SECONDS = 60
HEARTBEAT_SECONDS = 15
REQUEST_TIMEOUT = 10


class RelayError(RuntimeError):
    pass


def socket_path(session="default"):
    """Where a Herdr server listens for the given session name, on this host."""
    override = os.environ.get("HERDR_SOCKET_PATH")
    if override and session in ("", "default", os.path.basename(os.path.dirname(override))):
        return override
    base = os.path.join(os.path.expanduser("~"), ".config", "herdr")
    if session in ("", "default"):
        return os.path.join(base, "herdr.sock")
    return os.path.join(base, "sessions", session, "herdr.sock")


def _connect(path):
    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    connection.settimeout(REQUEST_TIMEOUT)
    try:
        connection.connect(path)
    except OSError as error:
        connection.close()
        raise RelayError("cannot reach Herdr at " + path + ": " + str(error)) from None
    return connection


def _readline(connection):
    chunks = bytearray()
    while True:
        try:
            chunk = connection.recv(4096)
        except socket.timeout:
            raise RelayError("Herdr did not answer within " + str(REQUEST_TIMEOUT) + "s") from None
        if not chunk:
            break
        chunks += chunk
        if b"\n" in chunks:
            line, _, rest = bytes(chunks).partition(b"\n")
            return line, rest
    return bytes(chunks), b""


def request(path, method, params):
    """One request on its own connection; the server closes mixed connections."""
    with _connect(path) as connection:
        connection.sendall((json.dumps({"id": "relay:" + uuid.uuid4().hex[:8], "method": method, "params": params}) + "\n").encode())
        line, _ = _readline(connection)
    try:
        response = json.loads(line or b"{}")
    except ValueError:
        raise RelayError("Herdr returned an unreadable response to " + method) from None
    if "error" in response:
        detail = response["error"]
        raise RelayError(detail.get("message", str(detail)) if isinstance(detail, dict) else str(detail))
    return response.get("result", response)


def subscribe(path, subscriptions):
    """A dedicated connection that only ever receives events for ``subscriptions``."""
    connection = _connect(path)
    try:
        connection.sendall((json.dumps({"id": "relay:sub", "method": "events.subscribe", "params": {"subscriptions": subscriptions}}) + "\n").encode())
        line, rest = _readline(connection)
        response = json.loads(line or b"{}")
    except (OSError, ValueError) as error:
        connection.close()
        raise RelayError("subscribe failed: " + str(error)) from None
    if response.get("result", {}).get("type") != "subscription_started":
        connection.close()
        detail = response.get("error", response)
        raise RelayError("subscribe refused: " + (detail.get("message", str(detail)) if isinstance(detail, dict) else str(detail)))
    connection.settimeout(None)
    connection.setblocking(False)
    return connection, rest


class Relay:
    def __init__(self, path, emit, commands=None, should_stop=lambda: False):
        self.path, self.emit, self.commands, self.should_stop = path, emit, commands, should_stop
        self.agents = {}
        self.watched = ()
        self.status_socket = None
        self.selector = selectors.DefaultSelector()
        self.buffers = {}

    # ----- state ---------------------------------------------------------

    def snapshot(self):
        result = request(self.path, "agent.list", {})
        agents = result.get("agents", []) if isinstance(result, dict) else []
        self.agents = {agent["pane_id"]: agent for agent in agents if agent.get("pane_id")}
        # Subscribe before announcing, so a reader acting on the snapshot never races the subscription.
        self.watch_status()
        self.emit({"type": "snapshot", "agents": list(self.agents.values())})

    def watch_status(self):
        """Hold one status subscription per known agent pane on a single connection, swapped atomically."""
        panes = tuple(sorted(self.agents))
        if panes == self.watched and (self.status_socket or not panes):
            return
        replacement = None
        if panes:
            replacement, rest = subscribe(self.path, [{"type": "pane.agent_status_changed", "pane_id": pane} for pane in panes])
            self.selector.register(replacement, selectors.EVENT_READ, "status")
            self.buffers[replacement] = bytearray(rest)
        if self.status_socket is not None:
            self._drop(self.status_socket)
        self.status_socket, self.watched = replacement, panes

    def _drop(self, connection):
        try:
            self.selector.unregister(connection)
        except (KeyError, ValueError):
            pass
        self.buffers.pop(connection, None)
        connection.close()

    def handle(self, message):
        event = message.get("event", "")
        data = message.get("data", {}) if isinstance(message.get("data"), dict) else {}
        if event == "pane.agent_status_changed":
            pane_id = data.get("pane_id")
            agent = self.agents.get(pane_id)
            if agent is None:
                self.snapshot()
                return
            for key in ("agent_status", "agent", "display_agent", "title", "state_labels", "workspace_id"):
                if data.get(key) is not None:
                    agent[key] = data[key]
            agent["state_change_seq"] = agent.get("state_change_seq", 0) + 1
            self.emit({"type": "agent", "agent": agent})
        elif event == "pane_updated":
            pane = data.get("pane", {})
            pane_id = pane.get("pane_id")
            if pane_id in self.agents:
                self.agents[pane_id].update({key: value for key, value in pane.items() if key != "state_change_seq"})
                self.emit({"type": "agent", "agent": self.agents[pane_id]})
            elif pane.get("agent"):
                self.snapshot()
        elif event in ("pane_closed", "pane_exited"):
            pane_id = data.get("pane_id") or data.get("pane", {}).get("pane_id")
            if pane_id in self.agents:
                del self.agents[pane_id]
                self.emit({"type": "gone", "pane_id": pane_id})
                self.watch_status()
        elif event in ("pane_created", "pane_agent_detected", "workspace_closed"):
            self.snapshot()

    # ----- loop ----------------------------------------------------------

    def run(self):
        lifecycle, rest = subscribe(self.path, [{"type": kind} for kind in LIFECYCLE])
        self.selector.register(lifecycle, selectors.EVENT_READ, "lifecycle")
        self.buffers[lifecycle] = bytearray(rest)
        if self.commands is not None:
            self.selector.register(self.commands, selectors.EVENT_READ, "commands")
        self.emit({"type": "hello", "socket": self.path})
        self.snapshot()
        last_reconcile = last_heartbeat = time.monotonic()
        try:
            while not self.should_stop():
                for key, _ in self.selector.select(timeout=1):
                    if key.data == "commands":
                        line = self.commands.readline()
                        if not line:
                            return
                        if line.strip() == "snapshot":
                            self.snapshot()
                        continue
                    connection = key.fileobj
                    try:
                        chunk = connection.recv(65536)
                    except (BlockingIOError, InterruptedError):
                        continue
                    if not chunk:
                        raise RelayError("Herdr closed the " + str(key.data) + " subscription")
                    buffer = self.buffers.setdefault(connection, bytearray())
                    buffer += chunk
                    while b"\n" in buffer:
                        line, _, remainder = bytes(buffer).partition(b"\n")
                        buffer[:] = remainder
                        if line.strip():
                            try:
                                self.handle(json.loads(line))
                            except ValueError:
                                continue
                now = time.monotonic()
                if now - last_heartbeat >= HEARTBEAT_SECONDS:
                    last_heartbeat = now
                    self.emit({"type": "ping"})
                if now - last_reconcile >= RECONCILE_SECONDS:
                    last_reconcile = now
                    self.snapshot()
        finally:
            for connection in list(self.buffers):
                self._drop(connection)
            self.selector.close()


def main(argv):
    session = argv[1] if len(argv) > 1 else "default"
    path = socket_path(session)
    stdout = os.fdopen(sys.stdout.fileno(), "w", buffering=1, closefd=False)

    def emit(message):
        stdout.write(json.dumps(message, separators=(",", ":")) + "\n")
        stdout.flush()

    try:
        Relay(path, emit, commands=sys.stdin).run()
    except RelayError as error:
        emit({"type": "error", "message": str(error)})
        return 1
    except (OSError, KeyboardInterrupt) as error:
        emit({"type": "error", "message": str(error) or "interrupted"})
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
