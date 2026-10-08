import io
import json
import os
import tempfile
import unittest
from unittest.mock import patch

from inbox import dictate, speech
from inbox.herdr import HerdrError, Machine
from inbox.store import Store


class FakeRecording:
    instances = []

    def __init__(self):
        self.path = tempfile.NamedTemporaryFile(suffix=".wav", delete=False).name
        self.stopped = self.cancelled = False
        FakeRecording.instances.append(self)

    def start(self):
        return self

    def elapsed(self):
        return 3

    def stop(self):
        self.stopped = True
        return self.path

    def cancel(self):
        self.cancelled = True
        os.unlink(self.path)


class FakeHerdr:
    def __init__(self, fail=""):
        self.calls, self.fail = [], fail

    def prompt(self, machine, pane_id, text, timeout_ms=15000):
        self.calls.append(("prompt", machine.id, pane_id, text))
        if self.fail:
            raise HerdrError(self.fail)

    def send_text(self, machine, pane_id, text):
        self.calls.append(("send_text", machine.id, pane_id, text))

    def raw(self, method, params, timeout=15):
        self.calls.append((method, params))
        return {"plugin_pane": {"pane": {"pane_id": "popup-1"}}}

    def agents(self, machine):
        return [{"pane_id": "w1:p2", "name": "claude", "tokens": {"thread": "Fix login"}}]


class DictateTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.env = patch.dict(os.environ, {"HERDR_PLUGIN_STATE_DIR": self.directory.name + "/state", "HERDR_PLUGIN_CONFIG_DIR": self.directory.name + "/config"})
        self.env.start()
        FakeRecording.instances = []
        self.patches = [patch("inbox.speech.Recording", FakeRecording), patch("inbox.speech.available_backends", return_value=["openai"]), patch("inbox.speech.transcribe", return_value="  fix the   login redirect ")]
        for item in self.patches:
            item.start()
        self.store = Store()
        self.machine = Machine("mac", "Mac Studio", "mac.local")

    def tearDown(self):
        for item in self.patches:
            item.stop()
        self.env.stop()
        self.directory.cleanup()

    def run_headless(self, herdr, command, machine=None):
        session = dictate.Session(herdr, self.store, machine or self.machine, "w1:p2", "Fix login")
        out = io.StringIO()
        code = dictate.headless(session, stdin=io.StringIO(command), stdout=out)
        return code, [json.loads(line) for line in out.getvalue().splitlines()]

    def test_enter_submits_the_normalized_transcript_as_a_prompt_on_the_pane_machine(self):
        herdr = FakeHerdr()
        code, events = self.run_headless(herdr, "send\n")
        self.assertEqual(code, 0)
        self.assertEqual([event["event"] for event in events], ["recording", "transcribing", "done"])
        self.assertEqual(events[0]["target"], "Fix login")
        self.assertEqual(events[-1], {"event": "done", "mode": "send", "text": "fix the login redirect"})
        self.assertEqual(herdr.calls, [("prompt", "mac", "w1:p2", "fix the login redirect")])
        recording, = FakeRecording.instances
        self.assertTrue(recording.stopped)
        self.assertFalse(os.path.exists(recording.path))

    def test_ctrl_t_types_the_transcript_without_submitting(self):
        herdr = FakeHerdr()
        code, events = self.run_headless(herdr, "type\n")
        self.assertEqual(code, 0)
        self.assertEqual(events[-1]["mode"], "type")
        self.assertEqual(herdr.calls, [("send_text", "mac", "w1:p2", "fix the login redirect")])

    def test_escape_or_a_closed_stdin_discards_the_recording(self):
        for command in ("cancel\n", "", "nonsense\n"):
            FakeRecording.instances = []
            herdr = FakeHerdr()
            code, events = self.run_headless(herdr, command)
            self.assertEqual(code, 0)
            self.assertEqual(events[-1], {"event": "cancelled"})
            self.assertTrue(FakeRecording.instances[0].cancelled)
            self.assertEqual(herdr.calls, [])

    def test_missing_transcription_service_fails_before_recording(self):
        with patch("inbox.speech.available_backends", return_value=[]):
            code, events = self.run_headless(FakeHerdr(), "send\n")
        self.assertEqual(code, 1)
        self.assertEqual(events[0]["event"], "error")
        self.assertIn("F10", events[0]["message"])
        self.assertEqual(FakeRecording.instances, [])

    def test_transcription_failure_is_reported_and_nothing_is_delivered(self):
        herdr = FakeHerdr()
        with patch("inbox.speech.transcribe", side_effect=speech.SpeechError("Transcription failed: 401")):
            code, events = self.run_headless(herdr, "send\n")
        self.assertEqual(code, 1)
        self.assertEqual(events[-1], {"event": "error", "message": "Transcription failed: 401"})
        self.assertEqual(herdr.calls, [])

    def test_a_blocked_agent_keeps_the_transcript_in_the_error(self):
        code, events = self.run_headless(FakeHerdr(fail="agent_blocked: waiting for approval"), "send\n")
        self.assertEqual(code, 1)
        self.assertIn("Fix login is waiting for your approval", events[-1]["message"])
        self.assertIn("Transcript: fix the login redirect", events[-1]["message"])

    def test_machine_ids_resolve_to_local_or_a_saved_profile(self):
        self.assertTrue(dictate.machine_from_id("").local)
        self.assertTrue(dictate.machine_from_id("local").local)
        self.assertTrue(dictate.machine_from_id("Local").local)
        self.assertEqual(dictate.machine_from_id("prof-1").id, "prof-1")
        self.assertFalse(dictate.machine_from_id("prof-1").local)

    def test_open_popup_targets_the_focused_pane_through_the_popup_environment(self):
        herdr = FakeHerdr()
        dictate.open_popup(herdr, "w1:p2")
        method, params = herdr.calls[-1]
        self.assertEqual(method, "plugin.pane.open")
        self.assertEqual((params["plugin_id"], params["entrypoint"], params["placement"], params["focus"]), ("lucasscariot.herdr-inbox", "dictate", "popup", True))
        self.assertEqual(params["env"], {"HERDR_INBOX_PANE": "w1:p2", "HERDR_INBOX_MACHINE": "local"})
        with self.assertRaisesRegex(HerdrError, "Focus an agent pane"):
            dictate.open_popup(herdr, "")

    def test_popup_title_comes_from_the_thread_token(self):
        self.assertEqual(dictate.pane_title(FakeHerdr(), self.machine, "w1:p2"), "Fix login")
        self.assertEqual(dictate.pane_title(FakeHerdr(), self.machine, "w9:p9"), "w9:p9")


if __name__ == "__main__":
    unittest.main()
