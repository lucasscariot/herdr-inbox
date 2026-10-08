"""Dictate into any agent pane, from anywhere in Herdr.

The composer's Ctrl+T only reaches the composer. This module gives the same
record-then-transcribe flow a target pane instead: speak, then Enter submits
the transcript to that agent through Herdr's prompt transport, Ctrl+T types it
without submitting so it can be edited in place, and Esc discards it.

Two front ends drive one ``Session``:

- ``main.py dictate --pane ID [--machine ID]`` is headless. It prints one JSON
  event per line on stdout and takes one command line on stdin (``send``,
  ``type`` or ``cancel``; EOF cancels). The inbox client runs it for its
  global Ctrl+T, on whichever machine the focused thread lives.
- ``main.py dictate --popup`` is a small prompt for Herdr's popup placement.
  ``main.py dictate --open`` opens it over the focused pane; that is what the
  ``dictate`` plugin action runs, so any Herdr client can bind a key to it.
"""

import curses
import json
import os
import sys
import time

from . import speech
from .herdr import HerdrError, Machine, local_machine
from .text import ellipsis

PLUGIN_ID = "lucasscariot.herdr-inbox"
MODES = ("send", "type")
NO_BACKEND = "No transcription service is connected. Open the composer and press F10 to connect one."
POPUP_WIDTH, POPUP_HEIGHT = 64, 7


def machine_from_id(machine_id):
    """The machine a saved-profile ID names; blank, ``local`` or ``Local`` is this host."""
    if not machine_id or machine_id in ("local", "Local"):
        return local_machine()
    return Machine(machine_id, machine_id)


class DeliveryError(HerdrError):
    """The transcript exists but could not reach the pane; the message carries it."""


class Session:
    """One recording for one pane. ``start`` records, ``finish`` transcribes and delivers."""

    def __init__(self, herdr, store, machine, pane_id, title=""):
        self.herdr, self.store, self.machine, self.pane_id = herdr, store, machine, pane_id
        self.title = title or pane_id
        self.recording = None

    def start(self):
        # Refuse before touching the microphone: a recording nobody can transcribe is a lost message.
        if not speech.available_backends(self.store.config, self.store.credentials()):
            raise speech.SpeechError(NO_BACKEND)
        self.recording = speech.Recording().start()
        return self

    def elapsed(self):
        return self.recording.elapsed() if self.recording else 0

    def finish(self, mode):
        """Stop recording and transcribe; ``send`` submits the text as a prompt, ``type`` types it unsent."""
        if mode not in MODES:
            raise ValueError("mode must be send or type")
        recording, self.recording = self.recording, None
        if recording is None:
            raise speech.SpeechError("Nothing is recording.")
        path = recording.stop()
        try:
            text = " ".join(speech.transcribe(path, self.store.config, self.store.credentials()).split())
        finally:
            try:
                os.unlink(path)
            except OSError:
                pass
        try:
            if mode == "send":
                self.herdr.prompt(self.machine, self.pane_id, text, timeout_ms=8000)
            else:
                self.herdr.send_text(self.machine, self.pane_id, text)
        except HerdrError as error:
            reason = str(error)
            if "blocked" in reason.lower():
                reason = self.title + " is waiting for your approval; answer it first"
            raise DeliveryError("Not sent: " + reason + ". Transcript: " + text) from None
        return text

    def cancel(self):
        recording, self.recording = self.recording, None
        if recording is not None:
            recording.cancel()


# ----- headless front end ------------------------------------------------------

def headless(session, stdin=sys.stdin, stdout=sys.stdout):
    """Record until stdin says ``send``, ``type`` or ``cancel``; report JSON events; return an exit code."""

    def emit(**event):
        stdout.write(json.dumps(event, ensure_ascii=False) + "\n")
        stdout.flush()

    try:
        session.start()
    except (speech.SpeechError, HerdrError) as error:
        emit(event="error", message=str(error))
        return 1
    emit(event="recording", target=session.title)
    command = (stdin.readline() or "").strip() or "cancel"
    if command not in MODES:
        session.cancel()
        emit(event="cancelled")
        return 0
    emit(event="transcribing")
    try:
        text = session.finish(command)
    except (speech.SpeechError, HerdrError) as error:
        emit(event="error", message=str(error))
        return 1
    emit(event="done", mode=command, text=text)
    return 0


# ----- popup front end ---------------------------------------------------------

def popup_request(pane_id, machine_id="local"):
    """The ``plugin.pane.open`` call that shows the popup over the focused pane."""
    if not pane_id:
        raise HerdrError("Focus an agent pane first, then dictate.")
    return {
        "plugin_id": PLUGIN_ID,
        "entrypoint": "dictation",
        "placement": "popup",
        "focus": True,
        "width": POPUP_WIDTH,
        "height": POPUP_HEIGHT,
        "env": {"HERDR_INBOX_PANE": pane_id, "HERDR_INBOX_MACHINE": machine_id or "local"},
    }


def open_popup(herdr, pane_id, machine_id="local"):
    return herdr.raw("plugin.pane.open", popup_request(pane_id, machine_id))


def pane_title(herdr, machine, pane_id):
    """What the inbox calls this pane, for the popup header; the pane ID when unknown."""
    try:
        agents = herdr.agents(machine)
    except HerdrError:
        return pane_id
    for agent in agents:
        if agent.get("pane_id") == pane_id:
            return agent.get("tokens", {}).get("thread") or agent.get("terminal_title_stripped") or agent.get("name") or pane_id
    return pane_id


def popup(screen, session):
    """Full-screen prompt: Enter sends, Ctrl+T types, Esc discards. Returns an exit code."""
    curses.raw()
    curses.curs_set(0)
    curses.set_escdelay(30)
    screen.timeout(100)
    warn = ok = dim = 0
    if curses.has_colors():
        curses.start_color()
        curses.use_default_colors()
        curses.init_pair(1, curses.COLOR_YELLOW, -1)
        curses.init_pair(2, curses.COLOR_GREEN, -1)
        warn, ok, dim = curses.color_pair(1) | curses.A_BOLD, curses.color_pair(2) | curses.A_BOLD, curses.A_DIM

    def draw(status, style, hint):
        screen.erase()
        height, width = screen.getmaxyx()
        lines = [("Dictating to " + ellipsis(session.title, max(8, width - 16)), curses.A_BOLD), (status, style), (hint, dim)]
        for row, (text, attribute) in enumerate(lines):
            if row < height:
                try:
                    screen.addnstr(row + (1 if height > 4 else 0), 2, text, max(1, width - 3), attribute)
                except curses.error:
                    pass
        screen.refresh()

    def wait_for_key():
        screen.timeout(-1)
        try:
            screen.get_wch()
        except curses.error:
            pass

    try:
        session.start()
    except (speech.SpeechError, HerdrError) as error:
        draw("✗ " + str(error), warn, "Press any key to close")
        wait_for_key()
        return 1
    mode = None
    while mode is None:
        seconds = int(session.elapsed())
        draw("● Recording  %d:%02d" % divmod(seconds, 60), warn, "Enter send   Ctrl+T type without sending   Esc discard")
        try:
            key = screen.get_wch()
        except curses.error:
            continue
        if key in ("\n", "\r", curses.KEY_ENTER):
            mode = "send"
        elif key == "\x14":
            mode = "type"
        elif key in ("\x1b", "\x03"):
            session.cancel()
            return 0
    draw("⟳ Transcribing…", dim, "")
    try:
        text = session.finish(mode)
    except (speech.SpeechError, HerdrError) as error:
        draw("✗ " + str(error), warn, "Press any key to close")
        wait_for_key()
        return 1
    draw(("✓ Sent: " if mode == "send" else "✓ Typed: ") + text, ok, "")
    time.sleep(0.6)
    return 0
