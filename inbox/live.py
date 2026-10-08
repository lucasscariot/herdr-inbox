"""Keep one live event link per machine and feed its messages to the UI thread.

Local links run the relay in-process against the session socket. Remote links
run the same relay source on the saved machine over a persistent SSH session
and read its JSON lines. Every link reconnects with backoff and reports its
state, so the inbox always knows whether a machine is live, connecting, or out
of reach.
"""

import json
import os
import subprocess
import threading
import time
from pathlib import Path

from . import relay
from .herdr import host_command

RELAY_SOURCE = Path(__file__).with_name("relay.py").read_text()
STALL_SECONDS = 3 * relay.HEARTBEAT_SECONDS
BACKOFF = (1, 2, 4, 8, 16, 30)
STABLE_SECONDS = 60


class Link(threading.Thread):
    """Supervises one machine's relay: runs it, restarts it, and forwards its messages."""

    def __init__(self, machine, queue, session_socket=None):
        super().__init__(name="link:" + machine.id, daemon=True)
        self.machine, self.queue, self.session_socket = machine, queue, session_socket
        self.stopping = threading.Event()
        self.process = None
        self.relay = None
        self.lock = threading.Lock()
        self.state, self.detail = "connecting", ""
        self.want_snapshot = False

    # ----- public ----------------------------------------------------------

    def stop(self):
        self.stopping.set()
        with self.lock:
            process = self.process
        if process is not None:
            try:
                process.stdin.close()
            except OSError:
                pass
            try:
                process.terminate()
            except OSError:
                pass

    def request_snapshot(self):
        with self.lock:
            process = self.process
        if process is not None:
            try:
                process.stdin.write("snapshot\n")
                process.stdin.flush()
            except (OSError, ValueError):
                pass
        else:
            self.want_snapshot = True

    # ----- supervision ------------------------------------------------------

    def emit(self, message):
        message["machine"] = self.machine.id
        self.queue.put(message)

    def set_state(self, state, detail=""):
        self.state, self.detail = state, detail
        self.emit({"type": "link", "state": state, "detail": detail})

    def run(self):
        attempt = 0
        while not self.stopping.is_set():
            started = time.monotonic()
            self.set_state("connecting")
            try:
                if self.machine.local:
                    self.run_local()
                else:
                    self.run_remote()
                detail = "link ended"
            except relay.RelayError as error:
                detail = str(error)
            except OSError as error:
                detail = str(error)
            if self.stopping.is_set():
                break
            attempt = 0 if time.monotonic() - started > STABLE_SECONDS else attempt + 1
            delay = BACKOFF[min(attempt, len(BACKOFF) - 1)]
            self.set_state("unavailable", detail + " · retrying in " + str(delay) + "s")
            self.stopping.wait(delay)

    def run_local(self):
        path = self.session_socket or relay.socket_path(self.machine.session)

        def emit(message):
            if message.get("type") == "hello":
                self.set_state("live")
            self.emit(message)

        def should_stop():
            if self.want_snapshot and self.relay is not None:
                self.want_snapshot = False
                self.relay.snapshot()
            return self.stopping.is_set()

        self.relay = relay.Relay(path, emit, should_stop=should_stop)
        try:
            self.relay.run()
        finally:
            self.relay = None

    def run_remote(self):
        argv = host_command(self.machine, ["python3", "-c", RELAY_SOURCE, self.machine.session or "default"])
        process = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, bufsize=1, start_new_session=True)
        with self.lock:
            self.process = process
        watchdog = threading.Thread(target=self.watch_stall, args=(process,), daemon=True)
        self.last_line = time.monotonic()
        watchdog.start()
        try:
            if self.want_snapshot:
                self.want_snapshot = False
                self.request_snapshot()
            for line in process.stdout:
                self.last_line = time.monotonic()
                line = line.strip()
                if not line.startswith("{"):
                    continue  # login banners and shell noise
                try:
                    message = json.loads(line)
                except ValueError:
                    continue
                if message.get("type") == "hello":
                    self.set_state("live")
                elif message.get("type") == "error":
                    raise relay.RelayError(message.get("message", "relay error"))
                self.emit(message)
        finally:
            with self.lock:
                self.process = None
            try:
                process.terminate()
            except OSError:
                pass
            stderr = ""
            try:
                stderr = process.stderr.read().strip()[-300:] if process.stderr else ""
                process.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired, ValueError):
                try:
                    process.kill()
                except OSError:
                    pass
            for stream in (process.stdin, process.stdout, process.stderr):
                try:
                    stream.close()
                except (OSError, ValueError, AttributeError):
                    pass
        if stderr and not self.stopping.is_set():
            raise relay.RelayError(stderr.splitlines()[-1])
        raise relay.RelayError("the SSH link closed")

    def watch_stall(self, process):
        """Kill a relay whose heartbeats stopped; the supervisor reconnects."""
        while process.poll() is None and not self.stopping.is_set():
            if time.monotonic() - self.last_line > STALL_SECONDS:
                try:
                    process.kill()
                except OSError:
                    pass
                return
            time.sleep(1)


class Links:
    """All machine links plus the queue the UI drains."""

    def __init__(self, machines, queue, session_socket=None):
        self.links = {}
        for machine in machines:
            link = Link(machine, queue, session_socket if machine.local else None)
            self.links[machine.id] = link
            link.start()

    def request_snapshot(self, machine_id=None):
        for identifier, link in self.links.items():
            if machine_id in (None, identifier):
                link.request_snapshot()

    def state(self, machine_id):
        link = self.links.get(machine_id)
        return (link.state, link.detail) if link else ("unavailable", "no link")

    def stop(self):
        for link in self.links.values():
            link.stop()
