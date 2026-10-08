"""Dictation: record from the microphone, then transcribe through a connected service.

Recording uses whatever is installed (PipeWire, ALSA, sox, ffmpeg). Transcription
goes to the service the user connected from the Dictation menu, to keys in
config.json or the environment, or to a local whisper.cpp build. Nothing here
blocks the UI: recording is a child process and transcription runs in a worker.
"""

import base64
import json
import os
import re
import shlex
import shutil
import signal
import subprocess
import tempfile
import time
import urllib.error
import urllib.request
import uuid
from pathlib import Path

SAMPLE_RATE = 16000


class SpeechError(RuntimeError):
    pass


# (executable, argv builder). The first installed recorder wins.
RECORDERS = (
    ("pw-record", lambda path: ["pw-record", "--rate", str(SAMPLE_RATE), "--channels", "1", "--format", "s16", path]),
    ("arecord", lambda path: ["arecord", "-q", "-f", "S16_LE", "-r", str(SAMPLE_RATE), "-c", "1", path]),
    ("parecord", lambda path: ["parecord", "--rate=" + str(SAMPLE_RATE), "--channels=1", "--format=s16le", "--file-format=wav", path]),
    ("rec", lambda path: ["rec", "-q", "-r", str(SAMPLE_RATE), "-c", "1", "-b", "16", path]),
    ("ffmpeg", lambda path: ["ffmpeg", "-loglevel", "error", "-y", "-f", "avfoundation" if os.uname().sysname == "Darwin" else "pulse", "-i", ":0" if os.uname().sysname == "Darwin" else "default", "-ac", "1", "-ar", str(SAMPLE_RATE), path]),
)

# Hosted services. ``verify`` is a cheap authenticated GET used to check a pasted key.
SERVICES = {
    "groq": {"label": "Groq Whisper", "detail": "free tier · fastest", "env": ("GROQ_API_KEY",), "model": "whisper-large-v3-turbo", "keys_url": "https://console.groq.com/keys", "verify": "https://api.groq.com/openai/v1/models"},
    "gemini": {"label": "Google Gemini", "detail": "free tier", "env": ("GEMINI_API_KEY", "GOOGLE_API_KEY"), "model": "gemini-2.5-flash", "keys_url": "https://aistudio.google.com/apikey", "verify": "https://generativelanguage.googleapis.com/v1beta/models?pageSize=1&key={key}"},
    "openai": {"label": "OpenAI", "detail": "pay per minute", "env": ("OPENAI_API_KEY",), "model": "gpt-4o-mini-transcribe", "keys_url": "https://platform.openai.com/api-keys", "verify": "https://api.openai.com/v1/models"},
    "mistral": {"label": "Mistral Voxtral", "detail": "pay per minute", "env": ("MISTRAL_API_KEY",), "model": "voxtral-mini-latest", "keys_url": "https://console.mistral.ai/api-keys", "verify": "https://api.mistral.ai/v1/models"},
    "deepgram": {"label": "Deepgram Nova", "detail": "pay per minute", "env": ("DEEPGRAM_API_KEY",), "model": "nova-3", "keys_url": "https://console.deepgram.com/", "verify": "https://api.deepgram.com/v1/projects"},
}
SETUP_HINT = "Press F10 to connect a transcription service or install local whisper.cpp."
ANSI = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")

# Where package managers and other dictation tools keep whisper.cpp models.
MODEL_DIRS = ("~/.local/share/herdr-inbox/whisper/models", "~/.local/share/whisper.cpp/models", "~/.cache/whisper.cpp", "~/.local/share/voxtype/models", "/usr/share/whisper.cpp/models", "/usr/local/share/whisper.cpp/models", "/opt/homebrew/share/whisper-cpp/models")


def _last_paragraph(output):
    """Transcribers print progress first; the transcript is the last paragraph."""
    lines = [ANSI.sub("", line).rstrip() for line in output.splitlines()]
    paragraphs = [block for block in "\n".join(lines).split("\n\n") if block.strip()]
    return paragraphs[-1].strip() if paragraphs else ""


def find_model(speech=None):
    """A whisper.cpp model file: the configured path, or the first one found in known places."""
    configured = (speech or {}).get("model", "")
    if isinstance(configured, str) and configured.endswith(".bin") and Path(configured).expanduser().is_file():
        return str(Path(configured).expanduser())
    for directory in MODEL_DIRS:
        found = sorted(Path(directory).expanduser().glob("ggml-*.bin"))
        if found:
            return str(found[0])
    return ""


# Locally installed transcribers, detected on PATH. Any OS, any package manager.
LOCAL_TOOLS = {
    "voxtype": {"label": "voxtype", "detail": "local push-to-talk daemon, transcribes files offline", "argv": lambda path, model: ["voxtype", "-q", "transcribe", path], "parse": _last_paragraph, "needs_model": False},
    "whisper-cli": {"label": "whisper.cpp", "detail": "whisper-cli on PATH", "argv": lambda path, model: ["whisper-cli", "-m", model, "-l", "auto", "-nt", "-np", "-f", path], "parse": lambda output: output.strip(), "needs_model": True},
    "whisper-cpp": {"label": "whisper.cpp", "detail": "whisper-cpp on PATH", "argv": lambda path, model: ["whisper-cpp", "-m", model, "-l", "auto", "-nt", "-np", "-f", path], "parse": lambda output: output.strip(), "needs_model": True},
}


def detected_tools(speech=None):
    """IDs of local transcribers that can run right now, in preference order."""
    tools = []
    for name, tool in LOCAL_TOOLS.items():
        if shutil.which(name) and (not tool["needs_model"] or find_model(speech)):
            tools.append(name)
    return tools

WHISPER_REPO = "https://github.com/ggml-org/whisper.cpp"
WHISPER_MODEL_URL = "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{model}.bin"
LOCAL_MODEL = "small"


def recorder_command(path):
    for executable, build in RECORDERS:
        if shutil.which(executable):
            return build(path)
    return None


class Recording:
    """One microphone capture, written to a temporary WAV file."""

    def __init__(self):
        self.path = os.path.join(tempfile.gettempdir(), "herdr-inbox-" + uuid.uuid4().hex + ".wav")
        self.process = None
        self.started = None

    def start(self):
        argv = recorder_command(self.path)
        if not argv:
            raise SpeechError("No microphone recorder found. Install pipewire (pw-record), alsa-utils (arecord), sox, or ffmpeg.")
        try:
            self.process = subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, start_new_session=True)
        except OSError as error:
            raise SpeechError("Could not start " + argv[0] + ": " + str(error)) from error
        self.started = time.monotonic()
        return self

    def elapsed(self):
        return time.monotonic() - self.started if self.started else 0

    def stop(self):
        """Stop the recorder gracefully so the WAV header is finalized, then return the file."""
        process, self.process = self.process, None
        if process is None:
            raise SpeechError("Nothing is recording.")
        if process.poll() is None:
            process.send_signal(signal.SIGINT)
            try:
                process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        if not os.path.exists(self.path) or os.path.getsize(self.path) < 1024:
            detail = (process.stderr.read().decode("utf-8", "replace").strip() if process.stderr else "")[-300:]
            self.cancel()
            raise SpeechError("The recording is empty. " + (detail or "Check the microphone and its input level."))
        return self.path

    def cancel(self):
        process, self.process = self.process, None
        if process is not None and process.poll() is None:
            process.kill()
            process.wait()
        try:
            os.unlink(self.path)
        except OSError:
            pass


# ----- configuration ---------------------------------------------------------

def settings(config, credentials=None):
    """Merge config.json's ``speech`` object with credentials saved from the Dictation menu."""
    speech = config.get("speech", {}) if isinstance(config, dict) else {}
    if not isinstance(speech, dict):
        raise SpeechError('"speech" in config.json must be an object')
    merged = dict(speech)
    saved = credentials if isinstance(credentials, dict) else {}
    keys = dict(speech.get("keys", {}) if isinstance(speech.get("keys"), dict) else {})
    keys.update(saved.get("keys", {}) if isinstance(saved.get("keys"), dict) else {})
    merged["keys"] = keys
    for name in ("backend", "command"):
        if isinstance(saved.get(name), str) and saved[name].strip():
            merged[name] = saved[name].strip()
    return merged


def key_for(backend, speech):
    keys = speech.get("keys", {})
    if isinstance(keys, dict) and isinstance(keys.get(backend), str) and keys[backend].strip():
        return keys[backend].strip()
    for variable in SERVICES.get(backend, {}).get("env", ()):
        if os.environ.get(variable, "").strip():
            return os.environ[variable].strip()
    return ""


def available_backends(config, credentials=None):
    """Backends usable right now, in the order they would be tried."""
    speech = settings(config, credentials)
    available = []
    if isinstance(speech.get("command"), str) and speech["command"].strip():
        available.append("command")
    available.extend(detected_tools(speech))
    available.extend(name for name in SERVICES if key_for(name, speech))
    forced = speech.get("backend", "")
    if forced:
        return [forced] if forced in available else []
    return available


def describe(config, credentials=None):
    """A short label of the active transcription path, or empty when none."""
    backends = available_backends(config, credentials)
    if not backends:
        return ""
    backend = backends[0]
    if backend in LOCAL_TOOLS:
        return LOCAL_TOOLS[backend]["label"] + " (local)"
    if backend == "command":
        command = settings(config, credentials).get("command", "")
        return "local whisper.cpp" if "whisper" in command else "local command"
    return SERVICES[backend]["label"]


# ----- hosted services --------------------------------------------------------

def _multipart(fields, file_field, path, content_type="audio/wav"):
    boundary = "----herdr-inbox-" + uuid.uuid4().hex
    body = bytearray()
    for name, value in fields.items():
        body += ("--" + boundary + "\r\nContent-Disposition: form-data; name=\"" + name + "\"\r\n\r\n" + str(value) + "\r\n").encode()
    with open(path, "rb") as stream:
        data = stream.read()
    body += ("--" + boundary + "\r\nContent-Disposition: form-data; name=\"" + file_field + "\"; filename=\"" + os.path.basename(path) + "\"\r\nContent-Type: " + content_type + "\r\n\r\n").encode()
    body += data + ("\r\n--" + boundary + "--\r\n").encode()
    return bytes(body), "multipart/form-data; boundary=" + boundary


USER_AGENT = "herdr-inbox/0.4 (+https://herdr.dev)"


def _request(url, data, headers, timeout=90, method=None):
    # Cloudflare in front of some APIs rejects urllib's default agent string outright (error 1010).
    headers = dict({"User-Agent": USER_AGENT, "Accept": "application/json"}, **headers)
    request = urllib.request.Request(url, data=data, headers=headers, method=method or ("POST" if data is not None else "GET"))
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            raw = response.read().decode("utf-8")
            return json.loads(raw) if raw.strip() else {}
    except urllib.error.HTTPError as error:
        with error:
            detail = error.read().decode("utf-8", "replace")[:300]
        try:
            parsed = json.loads(detail)
            detail = parsed.get("error", {}).get("message") or parsed.get("message") or parsed.get("err_msg") or detail
        except (ValueError, AttributeError):
            pass
        raise SpeechError("HTTP " + str(error.code) + ": " + str(detail).strip()) from None
    except (urllib.error.URLError, OSError, ValueError) as error:
        raise SpeechError(str(getattr(error, "reason", error))) from None


def _auth_headers(backend, key):
    if backend == "deepgram":
        return {"Authorization": "Token " + key}
    if backend == "gemini":
        return {}
    return {"Authorization": "Bearer " + key}


def verify_key(backend, key):
    """Check a pasted key with a cheap authenticated request. Raises SpeechError when rejected."""
    service = SERVICES.get(backend)
    if not service:
        raise SpeechError("Unknown service: " + str(backend))
    key = key.strip()
    if not key or any(c.isspace() for c in key):
        raise SpeechError("That does not look like an API key.")
    try:
        _request(service["verify"].replace("{key}", key), None, _auth_headers(backend, key), timeout=20)
    except SpeechError as error:
        raise SpeechError(service["label"] + " rejected the key: " + str(error)) from None
    return True


def transcribe_with(backend, path, speech, key=""):
    model = speech.get("model") or SERVICES.get(backend, {}).get("model", "")
    language = speech.get("language", "")
    if backend in LOCAL_TOOLS:
        tool = LOCAL_TOOLS[backend]
        try:
            result = subprocess.run(tool["argv"](path, find_model(speech)), stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=300)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise SpeechError(backend + " failed: " + str(error)) from None
        if result.returncode:
            raise SpeechError(backend + " failed: " + ANSI.sub("", result.stderr or result.stdout).strip()[-300:])
        return tool["parse"](result.stdout)
    if backend == "command":
        argv = [part.replace("{file}", path) for part in shlex.split(speech["command"])]
        if path not in argv:
            argv.append(path)
        try:
            result = subprocess.run(argv, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=300)
        except (OSError, subprocess.TimeoutExpired) as error:
            raise SpeechError("Transcription command failed: " + str(error)) from None
        if result.returncode:
            raise SpeechError("Transcription command failed: " + (result.stderr or result.stdout).strip()[-300:])
        return result.stdout.strip()
    if backend in ("openai", "groq", "mistral"):
        url = {"openai": "https://api.openai.com/v1/audio/transcriptions", "groq": "https://api.groq.com/openai/v1/audio/transcriptions", "mistral": "https://api.mistral.ai/v1/audio/transcriptions"}[backend]
        fields = {"model": model, "response_format": "json"}
        if language:
            fields["language"] = language
        body, content_type = _multipart(fields, "file", path)
        response = _request(url, body, dict(_auth_headers(backend, key), **{"Content-Type": content_type}))
        return (response.get("text") or "").strip()
    if backend == "deepgram":
        url = "https://api.deepgram.com/v1/listen?model=" + model + "&smart_format=true" + ("&language=" + language if language else "")
        with open(path, "rb") as stream:
            data = stream.read()
        response = _request(url, data, dict(_auth_headers(backend, key), **{"Content-Type": "audio/wav"}))
        try:
            return response["results"]["channels"][0]["alternatives"][0]["transcript"].strip()
        except (KeyError, IndexError, TypeError):
            raise SpeechError("Deepgram returned no transcript") from None
    if backend == "gemini":
        url = "https://generativelanguage.googleapis.com/v1beta/models/" + model + ":generateContent?key=" + key
        with open(path, "rb") as stream:
            audio = base64.b64encode(stream.read()).decode("ascii")
        prompt = "Transcribe this audio verbatim" + (" in " + language if language else "") + ". Reply with the transcript only, no quotes or commentary."
        payload = {"contents": [{"parts": [{"text": prompt}, {"inline_data": {"mime_type": "audio/wav", "data": audio}}]}]}
        response = _request(url, json.dumps(payload).encode(), {"Content-Type": "application/json"})
        try:
            return "".join(part.get("text", "") for part in response["candidates"][0]["content"]["parts"]).strip()
        except (KeyError, IndexError, TypeError):
            raise SpeechError("Gemini returned no transcript") from None
    raise SpeechError("Unknown speech backend: " + str(backend))


def transcribe(path, config, credentials=None):
    speech = settings(config, credentials)
    backends = available_backends(config, credentials)
    if not backends:
        forced = speech.get("backend", "")
        raise SpeechError(("Speech backend " + forced + " has no key. " if forced else "No transcription service is connected. ") + SETUP_HINT)
    backend = backends[0]
    try:
        text = transcribe_with(backend, path, speech, key_for(backend, speech))
    except SpeechError as error:
        raise SpeechError("Transcription failed: " + str(error)) from None
    if not text:
        raise SpeechError("Nothing was transcribed. Try speaking closer to the microphone.")
    return text


# ----- local whisper.cpp ----------------------------------------------------

def whisper_home():
    return Path(os.environ.get("HERDR_INBOX_WHISPER_DIR", str(Path.home() / ".local/share/herdr-inbox/whisper")))


def whisper_command(model=LOCAL_MODEL):
    """The command template for an installed local whisper.cpp, or empty."""
    home = whisper_home()
    binary, weights = home / "build/bin/whisper-cli", home / "models" / ("ggml-" + model + ".bin")
    if binary.is_file() and weights.is_file():
        return shlex.join([str(binary), "-m", str(weights), "-l", "auto", "-nt", "-np", "-f"]) + " {file}"
    return ""


def _run(argv, cwd, progress, label):
    progress(label)
    try:
        result = subprocess.run(argv, cwd=cwd, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=1800)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise SpeechError(label + " failed: " + str(error)) from None
    if result.returncode:
        raise SpeechError(label + " failed: " + (result.stderr or result.stdout).strip()[-400:])


def install_whisper(progress=lambda message: None, model=LOCAL_MODEL):
    """Build whisper.cpp in the user's data directory and download a model. Returns the command template."""
    for tool in ("git", "cmake"):
        if not shutil.which(tool):
            raise SpeechError("Local whisper needs " + tool + " installed, plus a C++ compiler.")
    if not (shutil.which("c++") or shutil.which("g++") or shutil.which("clang++")):
        raise SpeechError("Local whisper needs a C++ compiler (gcc or clang).")
    home = whisper_home()
    home.parent.mkdir(parents=True, exist_ok=True)
    if not (home / "CMakeLists.txt").is_file():
        _run(["git", "clone", "--depth", "1", WHISPER_REPO, str(home)], str(home.parent), progress, "Cloning whisper.cpp")
    if not (home / "build/bin/whisper-cli").is_file():
        _run(["cmake", "-B", "build", "-DCMAKE_BUILD_TYPE=Release", "-DWHISPER_BUILD_TESTS=OFF", "-DGGML_NATIVE=ON"], str(home), progress, "Configuring the build")
        _run(["cmake", "--build", "build", "--config", "Release", "-j", str(max(2, os.cpu_count() or 2)), "--target", "whisper-cli"], str(home), progress, "Compiling whisper.cpp (a minute or two)")
    weights = home / "models" / ("ggml-" + model + ".bin")
    if not weights.is_file():
        weights.parent.mkdir(parents=True, exist_ok=True)
        partial = weights.with_suffix(".part")
        url = WHISPER_MODEL_URL.replace("{model}", model)
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers={"User-Agent": "herdr-inbox"}), timeout=60) as response, open(partial, "wb") as stream:
                total, done = int(response.headers.get("Content-Length") or 0), 0
                while True:
                    chunk = response.read(1 << 20)
                    if not chunk:
                        break
                    stream.write(chunk)
                    done += len(chunk)
                    progress("Downloading the " + model + " model… " + (str(done * 100 // total) + "%" if total else str(done >> 20) + " MB"))
        except (urllib.error.URLError, OSError) as error:
            try:
                partial.unlink()
            except OSError:
                pass
            raise SpeechError("Model download failed: " + str(getattr(error, "reason", error))) from None
        os.replace(partial, weights)
    command = whisper_command(model)
    if not command:
        raise SpeechError("whisper.cpp did not produce a usable build")
    progress("Local whisper is ready")
    return command
