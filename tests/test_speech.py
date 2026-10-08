import io
import json
import os
import stat
import tempfile
import time
import unittest
from pathlib import Path
from unittest.mock import patch

from inbox import speech
from inbox.store import Store
from inbox.ui import PROJECT, UI


class FakeResponse(io.BytesIO):
    def __enter__(self):
        return self

    def __exit__(self, *args):
        self.close()


class SpeechTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.audio = Path(self.directory.name) / "clip.wav"
        self.audio.write_bytes(b"RIFF" + b"\0" * 2000)
        self.novox = patch("inbox.speech.detected_tools", return_value=[])
        self.novox.start()

    def tearDown(self):
        self.novox.stop()
        self.directory.cleanup()

    def test_recorder_prefers_pipewire_then_alsa(self):
        with patch("inbox.speech.shutil.which", side_effect=lambda name: "/usr/bin/" + name if name in ("pw-record", "arecord") else None):
            self.assertEqual(speech.recorder_command("/tmp/x.wav")[0], "pw-record")
        with patch("inbox.speech.shutil.which", side_effect=lambda name: "/usr/bin/arecord" if name == "arecord" else None):
            self.assertEqual(speech.recorder_command("/tmp/x.wav")[:3], ["arecord", "-q", "-f"])
        with patch("inbox.speech.shutil.which", return_value=None):
            self.assertIsNone(speech.recorder_command("/tmp/x.wav"))
            with self.assertRaisesRegex(speech.SpeechError, "No microphone recorder"):
                speech.Recording().start()

    def test_backends_come_from_config_keys_environment_or_a_command(self):
        with patch.dict(os.environ, {}, clear=True):
            self.assertEqual(speech.available_backends({}), [])
            self.assertEqual(speech.available_backends({"speech": {"keys": {"openai": "sk-x"}}}), ["openai"])
            self.assertEqual(speech.available_backends({"speech": {"command": "whisper-cli -f {file}", "keys": {"openai": "sk-x"}}}), ["command", "openai"])
            self.assertEqual(speech.available_backends({"speech": {"backend": "gemini", "keys": {"openai": "sk-x"}}}), [])
            with self.assertRaisesRegex(speech.SpeechError, "No transcription service"):
                speech.transcribe(str(self.audio), {})
        with patch.dict(os.environ, {"GROQ_API_KEY": "gsk", "GOOGLE_API_KEY": "g"}, clear=True):
            self.assertEqual(speech.available_backends({}), ["groq", "gemini"])

    def test_openai_style_upload_sends_multipart_audio_with_bearer_key(self):
        captured = {}

        def fake_urlopen(request, timeout=0):
            captured["url"], captured["headers"], captured["body"] = request.full_url, dict(request.headers), request.data
            return FakeResponse(json.dumps({"text": "  fix the login redirect  "}).encode())

        with patch("inbox.speech.urllib.request.urlopen", fake_urlopen), patch.dict(os.environ, {}, clear=True):
            text = speech.transcribe(str(self.audio), {"speech": {"keys": {"groq": "gsk-test"}, "language": "en"}})
        self.assertEqual(text, "fix the login redirect")
        self.assertEqual(captured["url"], "https://api.groq.com/openai/v1/audio/transcriptions")
        self.assertEqual(captured["headers"]["Authorization"], "Bearer gsk-test")
        self.assertIn(b'name="model"\r\n\r\nwhisper-large-v3-turbo', captured["body"])
        self.assertIn(b'name="language"\r\n\r\nen', captured["body"])
        self.assertIn(b'filename="clip.wav"', captured["body"])
        self.assertIn(b"RIFF", captured["body"])

    def test_http_errors_surface_the_service_message(self):
        import urllib.error

        def failing(request, timeout=0):
            raise urllib.error.HTTPError(request.full_url, 401, "Unauthorized", {}, io.BytesIO(json.dumps({"error": {"message": "Invalid API key"}}).encode()))

        with patch("inbox.speech.urllib.request.urlopen", failing), patch.dict(os.environ, {}, clear=True):
            with self.assertRaisesRegex(speech.SpeechError, "HTTP 401.*Invalid API key"):
                speech.transcribe(str(self.audio), {"speech": {"keys": {"openai": "bad"}}})

    def test_local_command_receives_the_file_and_returns_stdout(self):
        script = Path(self.directory.name) / "transcriber"
        script.write_text("#!/bin/sh\ntest -f \"$2\" && printf 'hello from %s\\n' \"$1\"\n")
        script.chmod(script.stat().st_mode | stat.S_IEXEC)
        text = speech.transcribe(str(self.audio), {"speech": {"command": str(script) + " local {file}"}})
        self.assertEqual(text, "hello from local")


class ConnectTests(unittest.TestCase):
    def setUp(self):
        self.novox = patch("inbox.speech.detected_tools", return_value=[])
        self.novox.start()

    def tearDown(self):
        self.novox.stop()

    def test_verify_key_uses_a_cheap_authenticated_request(self):
        captured = {}

        def fake_urlopen(request, timeout=0):
            captured["url"], captured["headers"], captured["method"] = request.full_url, dict(request.headers), request.get_method()
            return FakeResponse(b'{"data": []}')

        with patch("inbox.speech.urllib.request.urlopen", fake_urlopen):
            self.assertTrue(speech.verify_key("groq", " gsk-test "))
            self.assertEqual((captured["url"], captured["method"], captured["headers"]["Authorization"]), ("https://api.groq.com/openai/v1/models", "GET", "Bearer gsk-test"))
            self.assertEqual(captured["headers"]["User-agent"], speech.USER_AGENT)
            speech.verify_key("gemini", "g-key")
            self.assertIn("key=g-key", captured["url"])
            self.assertNotIn("Authorization", captured["headers"])
            speech.verify_key("deepgram", "dg")
            self.assertEqual(captured["headers"]["Authorization"], "Token dg")
        with self.assertRaisesRegex(speech.SpeechError, "does not look like"):
            speech.verify_key("openai", "two words")

        import urllib.error

        def rejecting(request, timeout=0):
            raise urllib.error.HTTPError(request.full_url, 401, "Unauthorized", {}, io.BytesIO(b'{"error": {"message": "Invalid API Key"}}'))

        with patch("inbox.speech.urllib.request.urlopen", rejecting):
            with self.assertRaisesRegex(speech.SpeechError, "Groq Whisper rejected the key: HTTP 401: Invalid API Key"):
                speech.verify_key("groq", "bad")

    def test_saved_credentials_win_over_config_and_are_private(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"HERDR_PLUGIN_STATE_DIR": directory + "/state", "HERDR_PLUGIN_CONFIG_DIR": directory + "/config"}, clear=False):
            store = Store()
            store.config["speech"] = {"keys": {"openai": "from-config"}}
            self.assertEqual(speech.available_backends(store.config, store.credentials()), ["openai"])
            store.save_credentials({"backend": "groq", "keys": {"groq": "saved"}})
            path = Path(directory) / "state" / "credentials.json"
            self.assertEqual(stat.S_IMODE(path.stat().st_mode), 0o600)
            self.assertEqual(speech.available_backends(store.config, store.credentials()), ["groq"])
            self.assertEqual(speech.describe(store.config, store.credentials()), "Groq Whisper")
            self.assertEqual(speech.key_for("groq", speech.settings(store.config, store.credentials())), "saved")
            store.save_credentials({"backend": "command", "command": "/opt/whisper/whisper-cli -f {file}"})
            self.assertEqual(speech.describe(store.config, store.credentials()), "local whisper.cpp")

    def test_local_tools_are_detected_and_preferred_over_hosted_keys(self):
        output = 'Loading audio file: "/tmp/x.wav"\nProcessing 176000 samples (11.00s)...\n\x1b[2m2026-10-08T11:25:13Z\x1b[0m INFO Using local whisper\n\nAnd so my fellow Americans,\nask not.\n'
        self.assertEqual(speech.LOCAL_TOOLS["voxtype"]["parse"](output), "And so my fellow Americans,\nask not.")
        self.assertEqual(speech._last_paragraph("Processing 10 samples...\n"), "Processing 10 samples...")
        self.assertEqual(speech._last_paragraph(""), "")
        self.novox.stop()
        try:
            with patch("inbox.speech.shutil.which", side_effect=lambda name: "/usr/bin/" + name if name in ("voxtype", "whisper-cli") else None), patch("inbox.speech.find_model", return_value=""), patch.dict(os.environ, {"GROQ_API_KEY": "k"}, clear=True):
                self.assertEqual(speech.available_backends({}), ["voxtype", "groq"])
                self.assertEqual(speech.available_backends({"speech": {"command": "x {file}"}}), ["command", "voxtype", "groq"])
                self.assertEqual(speech.describe({}, {"backend": "voxtype"}), "voxtype (local)")
            with patch("inbox.speech.shutil.which", side_effect=lambda name: "/usr/bin/whisper-cli" if name == "whisper-cli" else None), patch("inbox.speech.find_model", return_value="/models/ggml-base.bin"), patch.dict(os.environ, {}, clear=True):
                self.assertEqual(speech.available_backends({}), ["whisper-cli"])
                self.assertEqual(speech.LOCAL_TOOLS["whisper-cli"]["argv"]("/tmp/x.wav", "/models/ggml-base.bin")[:3], ["whisper-cli", "-m", "/models/ggml-base.bin"])
            with patch("inbox.speech.shutil.which", return_value=None), patch.dict(os.environ, {}, clear=True):
                self.assertEqual(speech.available_backends({}, {"backend": "voxtype"}), [])
        finally:
            self.novox.start()

    def test_whisper_command_requires_binary_and_model(self):
        with tempfile.TemporaryDirectory() as directory, patch.dict(os.environ, {"HERDR_INBOX_WHISPER_DIR": directory}):
            self.assertEqual(speech.whisper_command(), "")
            (Path(directory) / "build/bin").mkdir(parents=True)
            (Path(directory) / "build/bin/whisper-cli").write_text("")
            (Path(directory) / "models").mkdir()
            (Path(directory) / "models/ggml-small.bin").write_text("")
            command = speech.whisper_command()
            self.assertTrue(command.endswith("-f {file}"))
            self.assertIn("ggml-small.bin", command)


class DictationFlowTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.env = patch.dict(os.environ, {"HERDR_PLUGIN_STATE_DIR": self.directory.name + "/state", "HERDR_PLUGIN_CONFIG_DIR": self.directory.name + "/config"})
        self.env.start()
        self.novox = patch("inbox.speech.detected_tools", return_value=[])
        self.novox.start()
        self.ui = UI(None, None, Store(), "new", demo=True)

    def tearDown(self):
        self.ui.pool.shutdown(wait=True)
        self.novox.stop()
        self.env.stop()
        self.directory.cleanup()

    def settle(self):
        for _ in range(100):
            self.ui.collect()
            if not self.ui.pending:
                return
            time.sleep(0.02)

    def choose(self, field, value):
        self.ui.open_picker(field)
        self.ui.selection = next(index for index, choice in enumerate(self.ui.choices()) if choice[1] == value)
        self.ui.choose()

    def test_ctrl_t_records_then_inserts_the_transcript_at_the_cursor(self):
        ui = self.ui
        ui.task, ui.cursor = "Refactor", len("Refactor")
        ui.key("\x14")
        self.assertIsNotNone(ui.recording)
        self.assertIn("Recording", ui.message)
        ui.key("x")  # typing is ignored while the microphone is open
        ui.key("\x14")
        self.assertIsNone(ui.recording)
        self.assertTrue(ui.transcribing)
        self.settle()
        self.assertFalse(ui.transcribing)
        self.assertEqual(ui.task, "Refactor Preview transcript: describe the change you want.")
        self.assertEqual(ui.cursor, len(ui.task))

    def test_enter_while_recording_sends_the_transcribed_task(self):
        ui = self.ui
        self.choose(PROJECT, "cockpit")
        ui.key("\x14")
        ui.key("\n")
        self.settle()
        self.assertEqual(ui.task, "Preview transcript: describe the change you want.")
        self.assertIn("Preview only", ui.message)

    def test_escape_discards_the_recording(self):
        ui = self.ui
        ui.key("\x14")
        ui.key("\x1b")
        self.assertIsNone(ui.recording)
        self.assertFalse(ui.finished)
        self.assertEqual(ui.task, "")

    def test_dictation_fills_an_inbox_reply(self):
        ui = self.ui
        ui.view = "inbox"
        ui.inbox_selection = next(index for index, row in enumerate(ui.rows) if row["status"] == "done")
        ui.key("r")
        self.assertEqual(ui.entry.kind, "reply")
        ui.key("\x14")
        ui.key("\x14")
        self.settle()
        self.assertEqual(ui.entry.text, "Preview transcript: describe the change you want.")

    def test_dictation_menu_connects_a_service_with_a_hidden_key(self):
        ui = self.ui
        ui.key(__import__("curses").KEY_F10)
        self.assertEqual(ui.picker, "dictation")
        values = [value for _, value, _ in ui.choices()]
        self.assertEqual(values[:2], ["connect:groq", "connect:gemini"])
        self.assertIn("local", values)
        self.assertNotIn("disconnect", values)
        ui.selection = values.index("connect:groq")
        ui.choose()
        self.assertEqual(ui.entry.kind, "secret")
        for character in "gsk-secret":
            ui.key(character)
        ui.key("\n")
        self.settle()
        self.assertEqual(ui.demo_credentials, {"backend": "groq", "keys": {"groq": "gsk-secret"}})
        self.assertIn("Connected Groq Whisper", ui.message)
        self.assertEqual(ui.task, "")

    def test_custom_command_and_disconnect(self):
        ui = self.ui
        ui.open_dictation_menu()
        ui.choose_dictation("command")
        for character in "my-stt {file}":
            ui.key(character)
        ui.key("\n")
        self.assertEqual(ui.demo_credentials, {"backend": "command", "command": "my-stt {file}"})
        ui.choose_dictation("disconnect")
        self.assertEqual(ui.demo_credentials, {})


if __name__ == "__main__":
    unittest.main()
