import json
import os
import queue
import socket
import sys
import tempfile
import threading
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from inbox import live, relay
from inbox.herdr import Machine


class FakeHerdrServer(threading.Thread):
    """Speaks the public socket protocol: requests on short connections, events on dedicated ones.

    Like the real server, a connection that sends a second request after
    subscribing is closed.
    """

    def __init__(self, path):
        super().__init__(daemon=True)
        self.path = path
        self.agents = []
        self.subscribers = []
        self.lock = threading.Lock()
        self.calls = []
        self.server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.server.bind(path)
        self.server.listen(16)
        self.server.settimeout(0.2)
        self.stopping = threading.Event()
        self.start()

    def run(self):
        while not self.stopping.is_set():
            try:
                connection, _ = self.server.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            threading.Thread(target=self.serve, args=(connection,), daemon=True).start()

    def serve(self, connection):
        stream = connection.makefile("rb")
        subscribed = None
        while True:
            try:
                line = stream.readline()
            except OSError:
                break
            if not line:
                break
            request = json.loads(line)
            self.calls.append(request["method"])
            if subscribed is not None:
                self.hangup(connection)  # mixed use is refused, as upstream does
                return
            if request["method"] == "events.subscribe":
                subscribed = request["params"]["subscriptions"]
                with self.lock:
                    self.subscribers.append((connection, subscribed))
                connection.sendall((json.dumps({"id": request["id"], "result": {"type": "subscription_started"}}) + "\n").encode())
                continue
            if request["method"] == "agent.list":
                with self.lock:
                    result = {"type": "agent_list", "agents": [dict(agent) for agent in self.agents]}
                connection.sendall((json.dumps({"id": request["id"], "result": result}) + "\n").encode())
            else:
                connection.sendall((json.dumps({"id": request["id"], "error": {"code": "unknown", "message": "nope"}}) + "\n").encode())
            connection.close()
            return

    def push(self, event, data):
        """Deliver an event to every subscriber whose subscription matches."""
        with self.lock:
            targets = list(self.subscribers)
        for connection, subscriptions in targets:
            for subscription in subscriptions:
                kind = subscription["type"]
                matches = kind == event or kind.replace(".", "_") == event
                if kind == "pane.agent_status_changed" and matches:
                    matches = subscription.get("pane_id") == data.get("pane_id")
                if matches:
                    try:
                        connection.sendall((json.dumps({"event": event, "data": data}) + "\n").encode())
                    except OSError:
                        pass
                    break

    def subscriptions(self):
        with self.lock:
            return [subs for _, subs in self.subscribers]

    @staticmethod
    def hangup(connection):
        try:
            connection.shutdown(socket.SHUT_RDWR)
        except OSError:
            pass
        connection.close()

    def close_subscribers(self):
        with self.lock:
            for connection, _ in self.subscribers:
                self.hangup(connection)
            self.subscribers.clear()

    def stop(self):
        self.stopping.set()
        self.close_subscribers()
        self.server.close()


def drain(q, timeout=3, until=None):
    messages, end = [], time.time() + timeout
    while time.time() < end:
        try:
            messages.append(q.get(timeout=0.05))
        except queue.Empty:
            continue
        if until and until(messages):
            break
    return messages


class RelayTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.path = os.path.join(self.directory.name, "s.sock")
        self.server = FakeHerdrServer(self.path)
        self.server.agents = [{"pane_id": "w1:p1", "agent": "claude", "agent_status": "working", "state_change_seq": 4, "cwd": "/code/api"}]

    def tearDown(self):
        self.server.stop()
        self.directory.cleanup()

    def run_relay(self, seconds=1.5):
        messages, stop = [], threading.Event()
        worker = threading.Thread(target=lambda: relay.Relay(self.path, messages.append, should_stop=stop.is_set).run(), daemon=True)
        worker.start()
        return messages, stop, worker

    def test_relay_snapshots_then_streams_status_changes_without_polling(self):
        messages, stop, worker = self.run_relay()
        time.sleep(0.4)
        self.assertEqual([m["type"] for m in messages], ["hello", "snapshot"])
        self.assertEqual(messages[1]["agents"][0]["pane_id"], "w1:p1")
        status_subscriptions = [subs for subs in self.server.subscriptions() if subs and subs[0]["type"] == "pane.agent_status_changed"]
        self.assertEqual(status_subscriptions, [[{"type": "pane.agent_status_changed", "pane_id": "w1:p1"}]])
        self.server.push("pane.agent_status_changed", {"pane_id": "w1:p1", "workspace_id": "w1", "agent_status": "done", "agent": "claude", "title": "Fixed it"})
        time.sleep(0.3)
        self.assertEqual(messages[-1]["type"], "agent")
        self.assertEqual((messages[-1]["agent"]["agent_status"], messages[-1]["agent"]["title"], messages[-1]["agent"]["state_change_seq"]), ("done", "Fixed it", 5))
        calls_before = self.server.calls.count("agent.list")
        self.server.push("pane_closed", {"pane_id": "w1:p1", "workspace_id": "w1"})
        time.sleep(0.3)
        self.assertEqual(messages[-1], {"type": "gone", "pane_id": "w1:p1"})
        self.assertEqual(self.server.calls.count("agent.list"), calls_before)
        self.server.agents = [{"pane_id": "w2:p1", "agent": "codex", "agent_status": "idle", "state_change_seq": 1}]
        self.server.push("pane_agent_detected", {"pane_id": "w2:p1", "workspace_id": "w2", "agent": "codex"})
        time.sleep(0.4)
        self.assertEqual(messages[-1]["type"], "snapshot")
        self.assertEqual([a["pane_id"] for a in messages[-1]["agents"]], ["w2:p1"])
        self.assertEqual(self.server.subscriptions()[-1], [{"type": "pane.agent_status_changed", "pane_id": "w2:p1"}])
        stop.set()
        worker.join(3)
        self.assertFalse(worker.is_alive())

    def test_relay_reports_a_closed_subscription_as_an_error(self):
        outcome, stop = [], threading.Event()

        def run():
            try:
                relay.Relay(self.path, lambda m: None, should_stop=stop.is_set).run()
                outcome.append("returned")
            except relay.RelayError as error:
                outcome.append(str(error))

        worker = threading.Thread(target=run, daemon=True)
        worker.start()
        time.sleep(0.4)
        self.server.close_subscribers()
        worker.join(3)
        self.assertFalse(worker.is_alive())
        self.assertIn("closed the", outcome[0])

    def test_standalone_script_streams_json_lines_and_exits_on_stdin_eof(self):
        import subprocess
        source = Path(relay.__file__).read_text()
        env = dict(os.environ, HERDR_SOCKET_PATH=self.path)
        process = subprocess.Popen([sys.executable, "-c", source, "default"], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
        first, second = json.loads(process.stdout.readline()), json.loads(process.stdout.readline())
        self.assertEqual((first["type"], second["type"]), ("hello", "snapshot"))
        process.stdin.write("snapshot\n")
        process.stdin.flush()
        self.assertEqual(json.loads(process.stdout.readline())["type"], "snapshot")
        process.stdin.close()
        self.assertEqual(process.wait(timeout=5), 0)


class LinkTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.path = os.path.join(self.directory.name, "s.sock")
        self.server = FakeHerdrServer(self.path)
        self.server.agents = [{"pane_id": "w1:p1", "agent": "pi", "agent_status": "idle", "state_change_seq": 1}]
        self.queue = queue.Queue()

    def tearDown(self):
        self.server.stop()
        self.directory.cleanup()

    def test_local_link_reconnects_with_backoff_after_the_server_drops_it(self):
        with patch("inbox.live.BACKOFF", (0.2,)):
            link = live.Link(Machine("local", "Local"), self.queue, session_socket=self.path)
            link.start()
            messages = drain(self.queue, until=lambda ms: any(m["type"] == "snapshot" for m in ms))
            self.assertEqual([m["type"] for m in messages], ["link", "link", "hello", "snapshot"])
            self.assertEqual([m["state"] for m in messages[:2]], ["connecting", "live"])
            self.server.close_subscribers()
            messages = drain(self.queue, until=lambda ms: sum(m["type"] == "snapshot" for m in ms) >= 1 and any(m.get("state") == "unavailable" for m in ms))
            states = [m["state"] for m in messages if m["type"] == "link"]
            self.assertIn("unavailable", states)
            self.assertEqual(states[-1], "live")
            link.request_snapshot()
            self.assertTrue(any(m["type"] == "snapshot" for m in drain(self.queue, timeout=2, until=lambda ms: any(m["type"] == "snapshot" for m in ms))))
            link.stop()
            link.join(3)
            self.assertFalse(link.is_alive())

    def test_remote_link_runs_the_relay_source_over_the_host_command(self):
        machine = Machine("studio", "Mac Studio", "me@studio", "default")
        env = dict(os.environ, HERDR_SOCKET_PATH=self.path)

        def fake_host_command(target, argv, cwd=None):
            self.assertEqual(target, machine)
            self.assertEqual(argv[:2], ["python3", "-c"])
            self.assertIn("class Relay", argv[2])
            return [sys.executable, "-c", "import os, subprocess, sys; os.environ['HERDR_SOCKET_PATH']=" + repr(self.path) + "; sys.exit(subprocess.call([sys.executable, '-c'] + sys.argv[1:], stdin=sys.stdin))"] + argv[2:]

        with patch("inbox.live.host_command", fake_host_command), patch.dict(os.environ, env):
            link = live.Link(machine, self.queue)
            link.start()
            messages = drain(self.queue, until=lambda ms: any(m["type"] == "snapshot" for m in ms))
            self.assertEqual(messages[-1]["machine"], "studio")
            self.assertEqual([a["pane_id"] for a in messages[-1]["agents"]], ["w1:p1"])
            self.server.push("pane.agent_status_changed", {"pane_id": "w1:p1", "workspace_id": "w1", "agent_status": "working"})
            messages = drain(self.queue, until=lambda ms: any(m["type"] == "agent" for m in ms))
            self.assertEqual(messages[-1]["agent"]["agent_status"], "working")
            link.stop()
            link.join(5)
            self.assertFalse(link.is_alive())


if __name__ == "__main__":
    unittest.main()
