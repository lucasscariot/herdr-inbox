"""Mouse and keyboard terminal UI for launching and switching agent threads.

Two views share one loop: the composer (``new``) and the inbox. Launches run in
the background, so the composer is free for the next task immediately.
"""

import curses
import importlib
import os
import queue
import re
import shutil
import subprocess
import time
from concurrent.futures import ThreadPoolExecutor

from . import banner, speech
from .herdr import HerdrError
from .inventory import checkout_for, discover, models_fresh
from .live import Links
from .presets import Preset
from .text import age, cell_width, clip, ellipsis, pad, rank, task_layout, tilde, width_of
from .threads import GROUP_LABEL, STATUS_GLYPH, branch_name, inbox_rows, launch, resumable, resume, sort_rows

# Composer fields. Pickers open a searchable list; the others are buttons or text.
PROJECT, MACHINE, HARNESS, TASK, SEND, MODEL, PRESET, THINKING, SAVE, WORKSPACE, DICTATION = range(11)
PICKER_FIELDS = {PROJECT: "project", MACHINE: "machine", HARNESS: "harness", MODEL: "model", PRESET: "preset", THINKING: "thinking", WORKSPACE: "workspace", DICTATION: "dictation"}
PICKER_TITLE = {"project": "Project", "machine": "Machine", "harness": "Harness", "model": "Model", "preset": "Preset", "thinking": "Thinking", "workspace": "Workspace", "dictation": "Dictation"}
TAB_ORDER = [TASK, PRESET, HARNESS, MODEL, THINKING, SEND, MACHINE, WORKSPACE, PROJECT, SAVE]
SELECTOR_KEYS = {curses.KEY_F2: PROJECT, curses.KEY_F3: HARNESS, curses.KEY_F4: MODEL, curses.KEY_F6: MACHINE, curses.KEY_F7: PRESET, curses.KEY_F8: THINKING, curses.KEY_F9: WORKSPACE}
DICTATE_KEYS = ("\x14",)
CTRL_ENTER = "\x00ctrl-enter"
ENTER_KEYS = ("\n", "\r", curses.KEY_ENTER)
BACKSPACE_KEYS = (curses.KEY_BACKSPACE, "\x7f", "\b")
TASK_LIMIT = 32000
LAUNCH_NOTE_SECONDS = 25

ACCENT, OK, WARN, INFO = 1, 2, 3, 18
BANNER_PAIR = 4
STATUS_COLOR = {"blocked": WARN, "done": OK, "working": INFO, "unknown": 0, "idle": 0}


class Entry:
    """A one-line text field: preset names, branch names, replies, and filters."""

    def __init__(self, kind, prompt, text="", limit=64, placeholder=""):
        self.kind, self.prompt, self.text, self.limit, self.placeholder = kind, prompt, text, limit, placeholder
        self.cursor = len(text)

    def insert(self, text):
        text = "".join(c for c in text.replace("\n", " ") if c.isprintable())[:max(0, self.limit - len(self.text))]
        self.text = self.text[:self.cursor] + text + self.text[self.cursor:]
        self.cursor += len(text)

    def key(self, key):
        if key in BACKSPACE_KEYS:
            if self.cursor:
                self.text = self.text[:self.cursor - 1] + self.text[self.cursor:]
                self.cursor -= 1
        elif key == curses.KEY_DC:
            self.text = self.text[:self.cursor] + self.text[self.cursor + 1:]
        elif key == curses.KEY_LEFT:
            self.cursor = max(0, self.cursor - 1)
        elif key == curses.KEY_RIGHT:
            self.cursor = min(len(self.text), self.cursor + 1)
        elif key in (curses.KEY_HOME, "\x01"):
            self.cursor = 0
        elif key in (curses.KEY_END, "\x05"):
            self.cursor = len(self.text)
        elif key == "\x15":
            self.text, self.cursor = "", 0
        elif isinstance(key, str) and key.isprintable():
            self.insert(key)
        else:
            return False
        return True


class UI:
    def __init__(self, screen, herdr, store, view, demo=False):
        self.screen, self.herdr, self.store, self.view = screen, herdr, store, view
        self.demo = demo
        self.pool = ThreadPoolExecutor(max_workers=6)
        self.pending = []
        self.inventories, self.health, self.agents_by_machine = {}, {}, {}
        self.machines = herdr.machines() if not demo else demo_machines()
        self.project = store.preferences.get("last_project", "")
        self.machine = self.machines[0]
        self.harness = ""
        self.model, self.model_context, self.model_selections = "", None, {}
        self.thinking, self.preset_intent = "", ""
        self.workspace = {"mode": store.default_workspace_mode(), "path": "", "branch": ""}
        self.workspace_context = None
        self.entry, self.renaming = None, None
        self.reply_target, self.history_draft = None, ""
        self.recording, self.transcribing, self.speech_target, self.connecting = None, False, None, ""
        self.preferred_machine = store.preferences.get(self.project, {}).get("machine", "local")
        self.preferred_harness = store.preferences.get(self.project, {}).get("harness", "")
        self.task, self.cursor, self.field = "", 0, TASK
        self.task_width = 60
        self.history_index = None
        self.picker, self.query, self.selection = None, "", 0
        self.message, self.message_color, self.finished = "", WARN, False
        self.launches = []
        self.hits, self.anchors, self.task_cells = [], {}, []
        self.rows, self.inbox_selection, self.filter = [], 0, ""
        self.activity = {} if demo else store.activity()
        self.activity_dirty = False
        self.inbox_queue = queue.Queue()
        self.live_agents, self.link_state = {}, {}
        self.links = None
        self.pasting, self.paste_buffer = False, ""
        self.size = None
        self.dirty = True
        self.cursor_position = None
        self.banner_updated = time.monotonic()
        self.banner_elapsed = 0
        self.banner_pose = banner.pose(0)
        self.banner_visible = False
        for machine in self.machines:
            cached = demo_inventory(machine) if demo else store.cached_inventory(machine)
            if cached:
                self.inventories[machine.id] = cached
            self.health[machine.id] = "ready" if demo else "loading"
        self.restore_choice()
        if not demo:
            self.links = Links(self.machines, self.inbox_queue, os.environ.get("HERDR_SOCKET_PATH"))
        self.refresh()

    # ----- background work -------------------------------------------------

    @property
    def busy(self):
        return any(entry["state"] == "running" for entry in self.launches)

    def background(self, kind, machine, callback, tag=None):
        self.pending.append((kind, machine, self.pool.submit(callback), tag))

    def in_flight(self, kind, machine):
        return any(k == kind and m.id == machine.id for k, m, _, _ in self.pending)

    def refresh(self, force_models=False):
        self.store.refresh_presets()
        if self.demo:
            self.rows = demo_rows(self.machines)
            return
        for machine in self.machines:
            if not self.in_flight("inventory", machine):
                self.background("inventory", machine, lambda m=machine: discover(m, self.store, include_models=False), tag=force_models)
        self.refresh_agents(force=True)

    def refresh_agents(self, force=False):
        """Ask every live link for a fresh snapshot. Events keep the inbox current in between."""
        if self.links is not None and force:
            self.links.request_snapshot()

    def drain_events(self):
        """Apply relay messages from every machine, then rebuild the rows that changed."""
        touched = set()
        while True:
            try:
                message = self.inbox_queue.get_nowait()
            except queue.Empty:
                break
            machine_id, kind = message.get("machine"), message.get("type")
            agents = self.live_agents.setdefault(machine_id, {})
            if kind == "snapshot":
                agents.clear()
                agents.update({agent["pane_id"]: agent for agent in message.get("agents", []) if agent.get("pane_id")})
                touched.add(machine_id)
            elif kind == "agent":
                agent = message.get("agent", {})
                if agent.get("pane_id"):
                    agents[agent["pane_id"]] = agent
                    touched.add(machine_id)
            elif kind == "gone":
                if agents.pop(message.get("pane_id"), None) is not None:
                    touched.add(machine_id)
            elif kind == "link":
                self.link_state[machine_id] = message.get("state", "connecting"), message.get("detail", "")
                if message.get("state") == "unavailable":
                    agents.clear()
                    touched.add(machine_id)
                self.dirty = True
        if touched:
            self.rebuild_rows(touched)

    def rebuild_rows(self, machine_ids=None):
        records = self.store.threads()
        for machine in self.machines:
            if machine_ids is not None and machine.id not in machine_ids:
                continue
            rows = inbox_rows(machine, list(self.live_agents.get(machine.id, {}).values()), records, self.inventories.get(machine.id))
            self.agents_by_machine[machine.id] = rows
            self.track_activity(rows)
            for row in resumable(rows):
                if not any(k == "resume" and t == row["record"]["id"] for k, _, _, t in self.pending):
                    row["record"]["stage"] = "submitting"
                    self.background("resume", machine, lambda record=row["record"]: resume(self.herdr, self.store, record), tag=row["record"]["id"])
        self.rows = sort_rows([row for rows in self.agents_by_machine.values() for row in rows])
        self.dirty = True

    def collect(self):
        self.drain_events()
        for entry in list(self.pending):
            kind, machine, future, tag = entry
            if not future.done():
                continue
            self.pending.remove(entry)
            self.dirty = True
            try:
                result = future.result()
                if kind == "inventory":
                    self.inventories[machine.id] = result
                    self.health[machine.id] = "ready"
                    self.restore_choice()
                    if (tag or not models_fresh(result)) and not self.in_flight("models", machine):
                        self.background("models", machine, lambda m=machine: discover(m, self.store))
                elif kind == "models":
                    self.inventories[machine.id] = result
                elif kind == "resume":
                    self.notify("Sent the waiting task to " + result["title"], OK)
                    self.refresh_agents(force=True)
                elif kind == "launch":
                    tag.update(state="done", finished=time.monotonic(), message="answer the startup prompt in the thread, the task follows" if result.get("stage") == "startup_blocked" else "unverified · check the thread" if result.get("unverified") else "launched")
                    self.refresh_agents(force=True)
                elif kind == "focus":
                    if not machine.local:
                        self.notify("Thread selected on " + machine.label + ". Click its row in the Herdr sidebar.", INFO)
                    else:
                        self.message = ""
                elif kind == "reply":
                    self.notify("Reply sent to " + tag, OK)
                    self.refresh_agents(force=True)
                elif kind == "speech":
                    self.transcribing = False
                    self.insert_transcript(result, *tag)
                elif kind == "verify":
                    credentials = self.store.credentials() if not self.demo else getattr(self, "demo_credentials", {})
                    keys = dict(credentials.get("keys", {}))
                    keys[tag] = result
                    self.save_speech({"backend": tag, "keys": keys})
                    self.notify("Connected " + speech.SERVICES[tag]["label"] + ". Press Ctrl+T to dictate.", OK)
                elif kind == "install":
                    self.save_speech({"backend": "command", "command": result})
                    self.notify("Local whisper.cpp is ready. Press Ctrl+T to dictate.", OK)
            except Exception as error:
                if kind == "inventory":
                    self.health[machine.id] = "unavailable"
                    self.notify(machine.label + " is unavailable: " + str(error).splitlines()[0], WARN)
                elif kind == "launch":
                    tag.update(state="failed", finished=time.monotonic(), message=str(error).splitlines()[0])
                    self.notify("Launch failed on " + machine.label + ": " + str(error).splitlines()[0], WARN)
                    self.refresh_agents(force=True)
                elif kind == "resume":
                    self.notify("Could not send the waiting task on " + machine.label + ": " + str(error).splitlines()[0], WARN)
                elif kind == "speech":
                    self.transcribing = False
                    self.notify(str(error).splitlines()[0], WARN)
                elif kind == "verify":
                    self.notify(str(error).splitlines()[0] + "  Press F10 to try again.", WARN)
                elif kind == "install":
                    self.notify(str(error).splitlines()[0], WARN)
                elif kind == "reply" and "blocked" in str(error).lower():
                    self.notify("That thread is waiting for your approval. Open it to answer.", WARN)
                else:
                    self.notify(machine.label + ": " + str(error).splitlines()[0], WARN)

    def notify(self, message, color=WARN):
        self.message, self.message_color = message, color

    def track_activity(self, rows):
        now = time.time()
        for row in rows:
            key = row["machine"].id + ":" + row["pane_id"]
            previous = self.activity.get(key)
            if not previous:
                # First sighting: the change happened at an unknown time, so show no age yet.
                self.activity[key] = {"seq": row["sequence"], "status": row["status"]}
                self.activity_dirty = True
            elif previous.get("seq") != row["sequence"] or previous.get("status") != row["status"]:
                self.activity[key] = {"seq": row["sequence"], "status": row["status"], "at": now}
                self.activity_dirty = True
        if self.activity_dirty and not self.demo:
            try:
                self.store.save_activity(self.activity)
            except OSError:
                pass
            self.activity_dirty = False

    # ----- composer state --------------------------------------------------

    def project_names(self):
        names = {p["name"] for inventory in self.inventories.values() for p in inventory["projects"]}
        return sorted(names, key=lambda name: (name != self.store.preferences.get("last_project"), name.lower()))

    def copies(self):
        return [(machine, project) for machine in self.machines for project in self.inventories.get(machine.id, {}).get("projects", []) if project["name"] == self.project]

    def selected_project(self):
        return next((p for m, p in self.copies() if m.id == self.machine.id), None)

    def harnesses(self, machine=None):
        return self.inventories.get((machine or self.machine).id, {}).get("harnesses", [])

    def restore_choice(self):
        copies = self.copies()
        if not copies:
            return
        self.machine = next((m for m, _ in copies if m.id == self.preferred_machine), copies[0][0])
        harnesses = self.harnesses()
        preset = next((p for p in self.store.presets if p.name == self.preset_intent), None)
        self.harness = preset.harness if preset else self.preferred_harness if self.preferred_harness in harnesses else next(iter(harnesses), "")
        self.restore_model()
        if preset:
            self.model, self.thinking = preset.model, preset.thinking
        self.restore_workspace()

    def model_catalog(self):
        return self.inventories.get(self.machine.id, {}).get("models", {}).get(self.harness, {})

    def restore_model(self):
        context = self.project, self.machine.id, self.harness
        if context == self.model_context:
            return
        if self.model_context:
            self.model_selections[self.model_context] = self.model, self.thinking
        self.model_context = context
        self.model, self.thinking = self.model_selections.get(context, (self.store.remembered_model(self.project, self.machine, self.harness), self.store.remembered_thinking(self.project, self.machine, self.harness)))

    def restore_workspace(self):
        context = self.project, self.machine.id
        project = self.selected_project()
        if context == self.workspace_context or not project:
            return
        self.workspace_context = context
        mode = self.store.remembered_workspace(self.project)
        self.workspace = {"mode": mode, "path": checkout_for(project, project["path"])["path"], "branch": ""}

    def preset_name(self):
        matches = [p.name for p in self.store.presets if (p.harness, p.model, p.thinking) == (self.harness, self.model, self.thinking)]
        return self.preset_intent if self.preset_intent in matches else next(iter(matches), "")

    def thinking_visible(self):
        return bool(self.thinking or self.model_catalog().get("thinking"))

    def tab_order(self):
        return [field for field in TAB_ORDER if field != THINKING or self.thinking_visible()]

    def workspace_label(self):
        project = self.selected_project()
        if self.workspace["mode"] == "worktree":
            return "New worktree" + (" · " + self.workspace["branch"] if self.workspace["branch"] else "")
        if not project:
            return "Checkout"
        checkout = checkout_for(project, self.workspace.get("path"))
        kind = "Worktree" if checkout.get("linked") else "Checkout"
        return kind + (" · " + checkout["branch"] if checkout.get("branch") else "")

    def workspace_hint(self):
        project = self.selected_project()
        if not project:
            return "Choose a project to decide where it runs"
        if self.workspace["mode"] == "worktree":
            branch = self.workspace["branch"] or branch_name(self.task, {c["branch"] for c in project.get("checkouts", [])}) if self.task.strip() else self.workspace["branch"]
            return "git worktree of " + tilde(project["path"]) + (" as " + branch if branch else " named after the task")
        checkout = checkout_for(project, self.workspace.get("path"))
        return tilde(checkout["path"])

    # ----- pickers ---------------------------------------------------------

    def choices(self):
        query = self.query
        if self.picker == "preset":
            harnesses = self.harnesses()
            choices = [(p.name + ("  (not installed)" if self.health[self.machine.id] == "ready" and p.harness not in harnesses else ""), p.name, p.harness + " · " + p.model + (" · " + p.thinking if p.thinking else "")) for p in self.store.presets]
        elif self.picker == "thinking":
            choices = [("Default thinking", "", "")] + [(level.capitalize(), level, "") for level in self.model_catalog().get("thinking", [])]
            if self.thinking and self.thinking not in [value for _, value, _ in choices]:
                choices.append((self.thinking.capitalize() + "  (check support)", self.thinking, ""))
        elif self.picker == "project":
            paths = {}
            for machine in self.machines:
                for project in self.inventories.get(machine.id, {}).get("projects", []):
                    paths.setdefault(project["name"], []).append((machine, project))
            choices = []
            for name in self.project_names():
                copies = paths.get(name, [])
                worktrees = sum(1 for _, p in copies for c in p.get("checkouts", []) if c.get("linked"))
                detail = " · ".join(part for part in [", ".join(m.label for m, _ in copies) if len(copies) > 1 or copies and not copies[0][0].local else "", str(worktrees) + " worktree" + ("s" if worktrees != 1 else "") if worktrees else ""] if part)
                choices.append((name, name, detail))
        elif self.picker == "machine":
            choices = [(m.label + ("  (unavailable)" if self.health[m.id] == "unavailable" else "  (checking)" if self.health[m.id] == "loading" else ""), m.id, tilde(p["path"])) for m, p in self.copies()]
        elif self.picker == "model":
            catalog = self.model_catalog()
            choices = [("Default model", "", "")]
            choices += [(choice["label"], choice["id"], choice["id"] if choice["id"] != choice["label"] else "") for choice in catalog.get("choices", [])]
            if self.model and self.model not in [value for _, value, _ in choices]:
                choices.append((self.model, self.model, ""))
            ranked = rank(query, choices, key=lambda choice: choice[0] + " " + choice[1])
            if query.strip() and catalog.get("selectable") and query.strip() not in [value for _, value, _ in choices]:
                ranked.append(("Use " + query.strip(), query.strip(), "native model ID"))
            return ranked
        elif self.picker == "dictation":
            active = self.speech_backend()
            choices = []
            for name in speech.detected_tools(self.speech_settings()):
                tool = speech.LOCAL_TOOLS[name]
                choices.append((tool["label"] + " · " + name + ("  ✓ connected" if active == name else "  ✓ detected"), "tool:" + name, tool["detail"]))
            for name, service in speech.SERVICES.items():
                connected = active == name
                choices.append((service["label"] + ("  ✓ connected" if connected else ""), "connect:" + name, service["detail"] if not connected else "paste a new key"))
            local = speech.whisper_command()
            choices.append(("Build whisper.cpp here" + ("  ✓ connected" if active == "command" and "whisper" in self.speech_settings().get("command", "") else "  ✓ built" if local else ""), "local", "offline, no account" + ("" if local else " · needs git, cmake, a C++ compiler · ~470 MB model")))
            choices.append(("Custom command…" + ("  ✓ connected" if active == "command" and "whisper" not in self.speech_settings().get("command", "") else ""), "command", "any transcriber that prints text for {file}"))
            if active:
                choices.append(("Disconnect " + speech.describe(self.store.config, self.store.credentials()), "disconnect", "forget the saved key or command"))
        elif self.picker == "workspace":
            project = self.selected_project()
            choices = [("New worktree", "worktree", "branch named after the task"), ("New worktree, named…", "worktree:named", "type a branch name")]
            for checkout in (project or {}).get("checkouts", []):
                kind = "Worktree" if checkout.get("linked") else "Checkout"
                choices.append((kind + (" · " + checkout["branch"] if checkout.get("branch") else ""), "checkout:" + checkout["path"], tilde(checkout["path"])))
        else:
            choices = [(name.capitalize(), name, "") for name in self.harnesses()]
        return rank(query, choices, key=lambda choice: choice[0])

    def open_picker(self, field):
        if field == THINKING and not self.thinking_visible():
            return
        self.entry = None
        self.field = field
        self.picker, self.query = PICKER_FIELDS[field], ""
        current = {PROJECT: self.project, MACHINE: self.machine.id, HARNESS: self.harness, MODEL: self.model, PRESET: self.preset_name(), THINKING: self.thinking, WORKSPACE: "worktree" if self.workspace["mode"] == "worktree" else "checkout:" + self.workspace.get("path", ""), DICTATION: "connect:" + (self.speech_backend() or "")}.get(field, "")
        self.selection = next((index for index, choice in enumerate(self.choices()) if choice[1] == current), 0)

    def move_field(self, direction):
        self.picker, self.query, self.entry = None, "", None
        order = self.tab_order()
        current = order.index(self.field) if self.field in order else 0
        self.field = order[(current + direction) % len(order)]

    def choose(self):
        choices = self.choices()
        if not choices:
            return
        value = choices[min(self.selection, len(choices) - 1)][1]
        if self.picker == "preset":
            preset = next(p for p in self.store.presets if p.name == value)
            self.preset_intent = preset.name
            self.harness = self.preferred_harness = preset.harness
            self.restore_model()
            self.model, self.thinking = preset.model, preset.thinking
        elif self.picker == "project":
            self.project = value
            preference = self.store.preferences.get(self.project, {})
            self.preferred_machine = preference.get("machine", self.machine.id)
            self.preferred_harness = preference.get("harness", self.harness)
            self.restore_choice()
        elif self.picker == "machine":
            self.machine = next(m for m in self.machines if m.id == value)
            self.preferred_machine = self.machine.id
            harnesses = self.harnesses()
            if not self.preset_intent and self.harness not in harnesses:
                self.harness = next(iter(harnesses), "")
            self.preferred_harness = self.harness
            self.restore_workspace()
        elif self.picker == "model":
            self.preset_intent = ""
            self.model = value
            self.model_selections[self.model_context] = self.model, self.thinking
        elif self.picker == "thinking":
            self.preset_intent = ""
            self.thinking = value
            self.model_selections[self.model_context] = self.model, self.thinking
        elif self.picker == "dictation":
            self.picker, self.query = None, ""
            self.choose_dictation(value)
            return
        elif self.picker == "workspace":
            repo = (self.selected_project() or {}).get("path", self.workspace.get("path", ""))
            if value == "worktree":
                self.workspace = {"mode": "worktree", "path": repo, "branch": ""}
            elif value == "worktree:named":
                self.picker, self.query = None, ""
                self.workspace = {"mode": "worktree", "path": repo, "branch": self.workspace.get("branch", "")}
                self.entry = Entry("branch", "Branch: ", self.workspace["branch"], limit=80, placeholder="feature/name")
                self.field = WORKSPACE
                return
            else:
                self.workspace = {"mode": "checkout", "path": value[len("checkout:"):], "branch": ""}
        else:
            self.preset_intent = ""
            self.harness = value
            self.preferred_harness = value
        self.restore_model()
        if self.preset_intent:
            preset = next(p for p in self.store.presets if p.name == self.preset_intent)
            self.model, self.thinking = preset.model, preset.thinking
        self.picker, self.query = None, ""
        self.field = TASK
        self.message = ""

    # ----- presets ---------------------------------------------------------

    def begin_save(self, rename=None):
        if not rename and not self.model:
            self.notify("Choose a model before saving a preset. You can type its native ID.", INFO)
            self.open_picker(MODEL)
            return
        self.renaming = rename
        self.entry = Entry("name", "Name: ", rename.name if rename else "", placeholder="Name this preset…")
        self.picker, self.field = None, SAVE
        self.message = ""

    def save_preset(self):
        name = self.entry.text.strip() if self.entry else ""
        if not name:
            self.notify("Give this preset a name.", INFO)
            return
        preset = Preset(name, self.renaming.harness, self.renaming.model, self.renaming.thinking) if self.renaming else Preset(name, self.harness, self.model, self.thinking)
        try:
            self.store.save_preset(preset, self.renaming.name if self.renaming else None)
        except (ValueError, OSError) as error:
            self.notify(str(error), WARN)
            return
        if not self.renaming or self.preset_intent == self.renaming.name:
            self.preset_intent = name
        self.entry, self.renaming, self.field = None, None, TASK
        self.notify("Saved preset " + name, OK)

    def manage_preset(self, rename=False):
        choices = self.choices()
        if not choices:
            return
        name = choices[max(0, min(self.selection, len(choices) - 1))][1]
        preset = next(p for p in self.store.presets if p.name == name)
        if rename:
            self.begin_save(preset)
        else:
            try:
                self.store.delete_preset(name)
            except OSError as error:
                self.notify(str(error), WARN)
                return
            if self.preset_intent == name:
                self.preset_intent = ""
            self.notify("Removed preset " + name, OK)

    # ----- launching -------------------------------------------------------

    def start(self, keep_draft=False):
        project = self.selected_project()
        if not project:
            self.notify("Choose a project first.", INFO)
            self.open_picker(PROJECT)
        elif self.health[self.machine.id] != "ready":
            self.notify(self.machine.label + " is " + ("still being checked" if self.health[self.machine.id] == "loading" else "unavailable") + ". Choose another machine or press F5.", WARN)
        elif not self.harness or self.harness not in self.harnesses():
            self.notify((self.harness.capitalize() or "A harness") + " is not installed on " + self.machine.label + ". Choose another preset or machine.", WARN)
        elif not self.task.strip():
            self.notify("Write a task before launching.", INFO)
            self.field = TASK
        elif self.demo:
            self.notify("Preview only. No agent was launched.", INFO)
        else:
            machine, harness, task, model, thinking = self.machine, self.harness, self.task, self.model, self.thinking
            workspace, inventory = dict(self.workspace), self.inventories[self.machine.id]
            note = {"state": "running", "started": time.monotonic(), "finished": None, "project": project["name"], "harness": harness, "machine": machine.label, "title": " ".join(task.split())[:40], "message": "queued"}
            self.launches.insert(0, note)
            del self.launches[8:]

            def progress(text):
                note["message"] = text.rstrip(".")

            self.background("launch", machine, lambda: launch(self.herdr, self.store, machine, project, harness, task, progress=progress, model=model, inventory=inventory, thinking=thinking, workspace=workspace), tag=note)
            self.message = ""
            if not keep_draft:
                self.task, self.cursor, self.history_index = "", 0, None
                if self.workspace["mode"] == "worktree":
                    self.workspace["branch"] = ""
            self.field = TASK

    def recall_history(self, direction):
        history = self.store.history()
        if not history:
            return
        if self.history_index is None:
            if direction < 0:
                return
            self.history_draft, index = self.task, 0
        else:
            index = self.history_index + direction
        if index < 0:
            self.task, self.history_index = self.history_draft, None
        elif index < len(history):
            self.task, self.history_index = history[index], index
        self.cursor = len(self.task)

    # ----- dictation -------------------------------------------------------

    def speech_settings(self):
        return speech.settings(self.store.config, self.store.credentials())

    def speech_backend(self):
        backends = speech.available_backends(self.store.config, self.store.credentials())
        return backends[0] if backends else ""

    def open_dictation_menu(self):
        self.entry = None
        if self.view == "new":
            self.field = TASK
        self.picker, self.query = "dictation", ""
        current = "connect:" + (self.speech_backend() or "")
        self.selection = next((index for index, choice in enumerate(self.choices()) if choice[1] == current), 0)

    def choose_dictation(self, value):
        if value.startswith("connect:"):
            backend = value[len("connect:"):]
            service = speech.SERVICES[backend]
            self.connecting = backend
            self.entry = Entry("secret", service["label"] + " key: ", limit=400, placeholder="paste the key, Enter to verify")
            self.open_url(service["keys_url"])
            self.notify("Opened " + service["keys_url"] + " in your browser. Create a key there, paste it here, press Enter.", INFO)
        elif value.startswith("tool:"):
            name = value[len("tool:"):]
            self.save_speech({"backend": name})
            self.notify("Dictation uses " + speech.LOCAL_TOOLS[name]["label"] + " (" + name + "). Press Ctrl+T to dictate.", OK)
        elif value == "local":
            command = speech.whisper_command()
            if command:
                self.save_speech({"backend": "command", "command": command})
                self.notify("Dictation uses local whisper.cpp.", OK)
            elif self.in_flight("install", self.machines[0]):
                self.notify("whisper.cpp is still installing…", INFO)
            else:
                self.notify("Installing whisper.cpp…", INFO)

                def progress(text):
                    self.notify("⟳ " + text, INFO)
                    self.dirty = True

                self.background("install", self.machines[0], lambda: speech.install_whisper(progress) if not self.demo else "demo {file}")
        elif value == "command":
            self.entry = Entry("command", "Command: ", self.speech_settings().get("command", ""), limit=400, placeholder="whisper-cli -m model.bin -nt -f {file}")
        elif value == "disconnect":
            self.save_speech({})
            self.notify("Dictation disconnected.", OK)

    def save_speech(self, credentials):
        if self.demo:
            self.demo_credentials = credentials
            return
        try:
            self.store.save_credentials(credentials)
        except OSError as error:
            self.notify("Could not save: " + str(error), WARN)

    def open_url(self, url):
        if self.demo:
            return
        opener = next((tool for tool in ("xdg-open", "open") if shutil.which(tool)), None)
        if opener:
            try:
                subprocess.Popen([opener, url], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
            except OSError:
                pass

    def submit_secret(self):
        backend, key = self.connecting, self.entry.text.strip()
        if not key:
            self.notify("Paste the key first, or press Esc.", INFO)
            return
        self.entry = None
        self.notify("⟳ Checking the " + speech.SERVICES[backend]["label"] + " key…", INFO)
        self.background("verify", self.machines[0], lambda: (speech.verify_key(backend, key) if not self.demo else True) and key, tag=backend)

    def toggle_dictation(self, send=False):
        if self.recording:
            self.stop_dictation(send)
            return
        if self.transcribing:
            self.notify("Still transcribing the previous recording…", INFO)
            return
        if self.picker:
            self.picker = None
        if self.view == "new" and not self.entry:
            self.field = TASK
        target = "entry" if self.entry else "task" if self.view == "new" else None
        if target is None:
            self.notify("Press r to reply to a thread, then Ctrl+T to dictate the reply.", INFO)
            return
        if not self.demo and not self.speech_backend():
            self.notify("Connect a transcription service to dictate. Local whisper.cpp needs no account.", INFO)
            self.open_dictation_menu()
            return
        try:
            self.recording = speech.Recording().start() if not self.demo else DemoRecording()
        except speech.SpeechError as error:
            self.recording = None
            self.notify(str(error), WARN)
            return
        self.speech_target = target
        self.notify("● Recording — Enter stops and sends, Ctrl+T stops to edit, Esc cancels", WARN)

    def stop_dictation(self, send):
        recording, self.recording = self.recording, None
        try:
            path = recording.stop()
        except speech.SpeechError as error:
            self.notify(str(error), WARN)
            return
        self.transcribing = True
        self.notify("⟳ Transcribing…", INFO)
        target, config, credentials = self.speech_target, self.store.config, self.store.credentials()

        def work():
            try:
                return speech.transcribe(path, config, credentials) if not self.demo else "Preview transcript: describe the change you want."
            finally:
                try:
                    os.unlink(path)
                except OSError:
                    pass

        self.background("speech", self.machines[0], work, tag=(target, send))

    def cancel_dictation(self):
        recording, self.recording = self.recording, None
        recording.cancel()
        self.notify("Recording discarded.", INFO)

    def insert_transcript(self, text, target, send):
        text = " ".join(text.split())
        if not text:
            self.notify("Nothing was transcribed.", WARN)
            return
        if target == "entry" and self.entry:
            self.entry.insert((" " if self.entry.text and self.entry.cursor and not self.entry.text[self.entry.cursor - 1].isspace() else "") + text)
            if self.entry.kind == "filter":
                self.filter = self.entry.text
            self.message = ""
            if send:
                self.entry_key("\n")
            return
        before = self.task[:self.cursor]
        separator = " " if before and not before[-1].isspace() else ""
        text = (separator + text)[:max(0, TASK_LIMIT - len(self.task))]
        self.task = before + text + self.task[self.cursor:]
        self.cursor += len(text)
        self.history_index = None
        self.view, self.field, self.picker = "new", TASK, None
        self.message = ""
        if send:
            self.start()

    # ----- drawing primitives ---------------------------------------------

    def text(self, y, x, text, style=0, width=None):
        height, columns = self.screen.getmaxyx()
        if 0 <= y < height and 0 <= x < columns - 1:
            value = clip(text, min(width if width is not None else columns - x - 1, columns - x - 1))
            try:
                self.screen.addstr(y, x, value, style)
            except curses.error:
                pass

    def button(self, y, x, label, action, selected=False, style=None):
        style = curses.color_pair(ACCENT) | (curses.A_REVERSE if selected else curses.A_BOLD) if style is None else style
        self.text(y, x, label, style)
        self.hits.append((y, x, x + width_of(label), action))
        return width_of(label)

    def box(self, y, x, height, width, style=0):
        self.text(y, x, "╭" + "─" * (width - 2) + "╮", style)
        for line in range(y + 1, y + height - 1):
            self.text(line, x, "│", style)
            self.text(line, x + width - 1, "│", style)
        self.text(y + height - 1, x, "╰" + "─" * (width - 2) + "╯", style)

    def chip(self, y, x, label, field, width):
        prefix = PICKER_TITLE[PICKER_FIELDS[field]] + ": " if width >= 24 else ""
        label = "[ " + ellipsis(prefix + label, width - 6) + " ▾ ]"
        style = curses.color_pair(ACCENT) | curses.A_REVERSE | curses.A_BOLD if self.field == field and not self.entry else 0
        self.text(y, x, label, style)
        self.hits.append((y, x, x + width_of(label), ("field", field)))
        self.anchors[field] = (y, x)
        return width_of(label)

    def draw_entry(self, y, x, width):
        entry = self.entry
        available = max(1, width - width_of(entry.prompt) - 4)
        offset = 0
        while width_of(entry.text[offset:entry.cursor]) >= available:
            offset += 1
        body = ("•" * len(entry.text[offset:]) if entry.kind == "secret" else entry.text[offset:]) or entry.placeholder
        self.text(y, x, "[ " + entry.prompt, curses.color_pair(ACCENT))
        self.text(y, x + 2 + width_of(entry.prompt), clip(body, available), curses.A_DIM if not entry.text else 0, available)
        self.text(y, x + 2 + width_of(entry.prompt) + available, " ]", curses.color_pair(ACCENT))
        self.cursor_position = y, x + 2 + width_of(entry.prompt) + width_of(entry.text[offset:entry.cursor])

    def draw_header(self, height, width):
        title = "New thread" if self.view == "new" else "Agent inbox"
        self.text(0, 2, title, curses.A_BOLD)
        x = width - 2
        for machine in reversed(self.machines):
            link, _ = self.link_state.get(machine.id, ("live" if self.demo else "connecting", ""))
            state = "unavailable" if "unavailable" in (link, self.health.get(machine.id)) else "ready" if link == "live" and self.health.get(machine.id) == "ready" else "loading"
            glyph, color = ("●", OK) if state == "ready" else ("✗", WARN) if state == "unavailable" else ("◌", 0)
            label = machine.label
            x -= width_of(label) + 3
            if x < width_of(title) + 6:
                break
            self.text(0, x, glyph, curses.color_pair(color) if color else curses.A_DIM)
            self.text(0, x + 2, label, curses.A_DIM)

    def draw_footer(self, height, width, lines):
        for offset, line in enumerate(reversed(lines)):
            self.text(height - 1 - offset, 2, line, curses.A_DIM)

    def draw(self):
        self.screen.erase()
        self.cursor_position = None
        self.banner_visible = False
        self.hits, self.anchors = [], {}
        height, width = self.screen.getmaxyx()
        if height < 16 or width < 44:
            self.text(0, 0, "Enlarge this terminal to at least 44 x 16.")
            curses.curs_set(0)
            self.screen.refresh()
            return
        self.draw_header(height, width)
        if self.view == "new":
            self.draw_form(height, width)
        else:
            self.draw_inbox(height, width)
        if self.picker:
            self.draw_picker(height, width)
        # Set the terminal cursor after rendering footers and menus: curses
        # otherwise leaves it at the last text drawn, away from the prompt.
        curses.curs_set(1 if self.cursor_position else 0)
        if self.cursor_position:
            try:
                self.screen.move(*self.cursor_position)
            except curses.error:
                pass
        self.screen.refresh()

    # ----- composer --------------------------------------------------------

    def draw_form(self, height, width):
        box_width = min(100, width - 8)
        stacked = box_width < 70
        x = (width - box_width) // 2
        task_width = box_width - 6
        self.task_width = task_width
        wrapped, (cursor_row, cursor_column), positions = task_layout(self.task, self.cursor, task_width)
        thinking_visible = self.thinking_visible()
        notes = self.visible_launches()
        extra = int(stacked) + min(3, len(notes)) + int(bool(self.message))
        task_height = min(max(4, len(wrapped)), max(3, min(10, height - 12 - extra)))
        offset = max(0, min(cursor_row - task_height + 1, len(wrapped) - task_height))
        block = task_height + 8 + extra
        y = max(4, (height - block) // 2)
        # Give the sculpture every row the form does not need, so it balances the wordmark.
        art_height = min(24, max(12, height - 24 - extra))
        if height >= 38 and width >= 80:
            y = max(y, art_height + 8)
        if width >= 80 and y >= 20:
            art_width = min(108, width - 8)
            art_x, art_y = (width - art_width) // 2, y - art_height - 5
            self.banner_visible = True
            for row, runs in enumerate(banner.frame(art_width, art_height, *self.banner_pose)):
                for column, text, shade in runs:
                    self.text(art_y + row, art_x + column, text, curses.color_pair(BANNER_PAIR + shade))
        elif y >= 9:
            title = "[ HERDR INBOX ]"
            self.text(y - 6, (width - len(title)) // 2, title, curses.color_pair(ACCENT))
        hero = "What should we build in"
        project_label = self.project or "Choose a project"
        hero_width = len(hero) + min(width_of(project_label), box_width - len(hero) - 7) + 6
        hero_x = max(x, (width - hero_width) // 2)
        self.text(y - 3, hero_x, hero, curses.A_BOLD)
        self.chip(y - 3, hero_x + len(hero) + 1, project_label, PROJECT, box_width - len(hero) - 1)
        save = "[ Save preset ]" if self.entry and self.entry.kind == "name" else "[ + Save preset ]"
        save_x = x + box_width - len(save) - 2
        if self.entry and self.entry.kind == "name":
            self.draw_entry(y - 1, x + 2, save_x - x - 4)
        else:
            self.chip(y - 1, x + 2, self.preset_name() or "Choose a preset", PRESET, box_width - len(save) - 6)
        self.button(y - 1, save_x, save, ("save",), self.field == SAVE, curses.color_pair(ACCENT) | (curses.A_REVERSE if self.field == SAVE else 0))
        prompt_focus = self.field == TASK and not self.picker and not self.entry
        border_style = curses.color_pair(ACCENT) if prompt_focus else curses.A_DIM
        self.box(y, x, task_height + 3 + int(stacked), box_width, border_style)
        self.text(y, x + 2, " Task ", border_style | curses.A_BOLD)
        self.anchors[DICTATION] = (y + 1, x + 2)
        if self.recording and self.speech_target == "task":
            seconds = int(self.recording.elapsed())
            marker = " ● Recording " + str(seconds // 60) + ":" + str(seconds % 60).zfill(2) + " "
            self.text(y, x + box_width - len(marker) - 2, marker, curses.color_pair(WARN) | curses.A_BOLD)
        elif self.transcribing and self.speech_target == "task":
            marker = " ⟳ Transcribing… "
            self.text(y, x + box_width - len(marker) - 2, marker, curses.color_pair(INFO) | curses.A_BOLD)
        elif self.history_index is not None:
            marker = " history " + str(self.history_index + 1) + "/" + str(len(self.store.history())) + " "
            self.text(y, x + box_width - len(marker) - 2, marker, curses.A_DIM)
        if not self.task:
            self.text(y + 1, x + 3, "Describe a task, ask for changes…  Ctrl+T dictates · Ctrl+P recalls a previous task", curses.A_DIM, task_width)
        else:
            for index, line in enumerate(wrapped[offset:offset + task_height]):
                self.text(y + 1 + index, x + 3, line, width=task_width)
        self.hits.extend((row, x + 1, x + box_width - 1, ("field", TASK)) for row in range(y + 1, y + task_height + 1))
        self.task_cells = [(y + 1 + row - offset, x + 3 + column, index) for index, (row, column) in enumerate(positions) if offset <= row < offset + task_height]
        if prompt_focus:
            self.cursor_position = y + 1 + cursor_row - offset, x + 3 + cursor_column
        controls_y = y + task_height + 1
        send = "[ Send ↑ ]"
        send_x = x + box_width - len(send) - 3
        model_label = next((choice["label"] for choice in self.model_catalog().get("choices", []) if choice["id"] == self.model), self.model or "Default model")
        cx = x + 2
        cx += self.chip(controls_y, cx, self.harness.capitalize() or "Choose harness", HARNESS, box_width - 4 if stacked else 30) + 1
        if stacked:
            controls_y += 1
            cx = x + 2
            cx += self.chip(controls_y, cx, model_label, MODEL, send_x - cx - 2 - (18 if thinking_visible else 0)) + 1
        else:
            cx += self.chip(controls_y, cx, model_label, MODEL, min(34, send_x - cx - 2 - (24 if thinking_visible else 0))) + 1
        if thinking_visible:
            self.chip(controls_y, cx, self.thinking.capitalize() or "Default", THINKING, send_x - cx - 2)
        self.button(controls_y, send_x, send, ("launch",), self.field == SEND, curses.color_pair(ACCENT) | curses.A_BOLD | (curses.A_REVERSE if self.field == SEND else 0))
        context_y = y + task_height + 3 + int(stacked)
        cx = x + 2
        cx += self.chip(context_y, cx, self.machine.label, MACHINE, min(28, box_width - 8)) + 2
        if self.entry and self.entry.kind in ("branch", "secret", "command"):
            self.draw_entry(context_y, cx, box_width - (cx - x) - 2)
        else:
            cx += self.chip(context_y, cx, self.workspace_label(), WORKSPACE, min(40, box_width - (cx - x) - 4)) + 2
            self.text(context_y, cx, self.workspace_hint(), curses.A_DIM, box_width - (cx - x) - 2)
        line = context_y + 1
        for note in notes[:3]:
            glyph, color = ("⟳", INFO) if note["state"] == "running" else ("✓", OK) if note["state"] == "done" else ("✗", WARN)
            label = glyph + " " + note["project"] + " · " + note["harness"] + " · " + note["title"] + " — " + note["message"]
            self.text(line, x + 2, label, curses.color_pair(color), box_width - 4)
            line += 1
        if self.message:
            self.text(line, x + 2, self.message.splitlines()[0], curses.color_pair(self.message_color) if self.message_color else 0, box_width - 4)
        if height >= 24:
            if self.recording:
                hints = ["Speak now   Enter stop and send   Ctrl+T stop and edit   Esc discard"]
            elif self.entry and self.entry.kind == "name":
                hints = ["Enter save preset   Esc cancel"]
            elif self.entry and self.entry.kind == "branch":
                hints = ["Enter use this branch name   Esc cancel   Leave empty to name it after the task"]
            elif self.entry and self.entry.kind == "secret":
                hints = ["Paste the key (it stays hidden)   Enter verify and save   Esc cancel"]
            elif self.entry and self.entry.kind == "command":
                hints = ["Enter save command   Esc cancel   {file} is replaced by the recording"]
            else:
                hints = ["Enter send   Ctrl+Enter send and keep draft   Ctrl+T dictate   Alt+Enter newline   Tab next field   Ctrl+P history   F5 refresh"]
            hints.append("F2 project   F3 harness   F4 model   F6 machine   F7 preset   F8 thinking   F9 workspace   F10 dictation   Ctrl+D save preset   Esc close" if width >= 132 else "F2 project F3 harness F4 model F6 machine F7 preset F8 thinking F9 workspace F10 dictation Ctrl+D save")
            self.draw_footer(height, width, hints)
        else:
            self.draw_footer(height, width, ["Tab move  Enter send  F7 preset  F9 workspace  Esc close"])

    def visible_launches(self):
        now = time.monotonic()
        return [note for note in self.launches if note["state"] == "running" or note["finished"] and now - note["finished"] < (LAUNCH_NOTE_SECONDS if note["state"] == "done" else LAUNCH_NOTE_SECONDS * 4)]

    def draw_picker(self, height, width):
        field = next(field for field, name in PICKER_FIELDS.items() if name == self.picker)
        anchor_y, anchor_x = self.anchors.get(field, (3, 2))
        choices = self.choices()
        self.selection = max(0, min(self.selection, len(choices) - 1))
        capacity = min(9, max(2, height - 10))
        box_height = min(capacity, max(1, len(choices))) + 5
        box_width = min(64, width - 6)
        x = min(anchor_x, width - box_width - 3)
        y = max(2, min(anchor_y + 1, height - box_height - 2))
        if field not in (PROJECT, PRESET, DICTATION) and anchor_y - box_height >= 2:
            y = anchor_y - box_height
        for row in range(y, y + box_height):
            self.text(row, x, " " * (box_width + 1))
        self.box(y, x, box_height, box_width, curses.color_pair(ACCENT))
        title = " " + PICKER_TITLE[self.picker] + " "
        self.text(y, x + 2, title, curses.color_pair(ACCENT) | curses.A_BOLD)
        self.text(y + 1, x + 2, "› " + (self.query or "Type to search…"), curses.A_BOLD if self.query else curses.A_DIM, box_width - 4)
        self.cursor_position = y + 1, x + 4 + width_of(clip(self.query, box_width - 8))
        offset = max(0, self.selection - capacity + 1)
        # The open menu owns clicks; clicking outside dismisses it.
        self.hits = []
        inner = box_width - 4
        for index, (label, value, detail) in enumerate(choices[offset:offset + capacity], offset):
            row = y + 2 + index - offset
            selected = index == self.selection
            detail_width = min(width_of(detail), max(0, inner - width_of(label) - 3)) if detail else 0
            line = pad(label, inner - detail_width - (1 if detail_width else 0)) + (" " + ellipsis(detail, detail_width) if detail_width else "")
            self.text(row, x + 2, pad(line, inner), curses.A_REVERSE if selected else 0, inner)
            if detail_width and not selected:
                self.text(row, x + 2 + inner - detail_width, ellipsis(detail, detail_width), curses.A_DIM, detail_width)
            self.hits.append((row, x + 1, x + box_width - 1, ("pick", index)))
        if not choices:
            loading_models = self.picker in ("model", "thinking") and self.in_flight("models", self.machine)
            empty = "No presets yet. Choose a model, then Ctrl+D." if self.picker == "preset" and not self.store.presets else "Loading models…" if loading_models else "Discovering…" if any(h == "loading" for h in self.health.values()) else "No matches"
            self.text(y + 2, x + 2, empty, curses.A_DIM, inner)
        help_text = "Enter use   Ctrl+R rename   Del remove   Esc back" if self.picker == "preset" else "↑↓ move   Enter select   Tab next   Esc back"
        self.text(y + box_height - 2, x + 2, help_text, curses.A_DIM, inner)

    # ----- inbox -----------------------------------------------------------

    def filtered_rows(self):
        if not self.filter.strip():
            return self.rows
        return rank(self.filter, self.rows, key=lambda row: " ".join([row["title"], row["project"], row["branch"], row["harness"], row["machine"].label]))

    def draw_inbox(self, height, width):
        rows = self.filtered_rows()
        self.button(2, 2, "[ + New thread ]", ("new",))
        counts = {}
        for row in self.rows:
            counts[row["status"]] = counts.get(row["status"], 0) + 1
        summary = str(len(self.rows)) + " thread" + ("s" if len(self.rows) != 1 else "")
        if counts.get("blocked"):
            summary += " · " + str(counts["blocked"]) + " need" + ("s" if counts["blocked"] == 1 else "") + " input"
        if counts.get("done"):
            summary += " · " + str(counts["done"]) + " ready"
        if self.filter.strip():
            summary = "filter: " + self.filter.strip() + " · " + str(len(rows)) + " of " + summary
        self.text(2, 20, summary, curses.A_DIM, width - 22)
        self.inbox_selection = max(0, min(self.inbox_selection, len(rows) - 1))
        two_line = width < 96
        footer_rows = 4 if self.entry else 3
        top, bottom = 4, height - footer_rows
        line_height = 2 if two_line else 1
        # Layout rows with group headers, then scroll to keep the selection visible.
        lines, group = [], None
        for index, row in enumerate(rows):
            if row["status"] != group:
                group = row["status"]
                lines.append(("group", group))
            lines.append(("row", index))
        selected_line = next((i for i, (kind, value) in enumerate(lines) if kind == "row" and value == self.inbox_selection), 0)
        capacity = bottom - top
        start = 0
        while sum(line_height if kind == "row" else 1 for kind, _ in lines[start:selected_line + 1]) > capacity:
            start += 1
        y = top
        title_width = max(16, width - 4 - 3 - (0 if two_line else 24 + 2 + 22 + 2 + 5)) if two_line else max(16, min(64, width - 4 - 3 - (24 + 2 + 22 + 2 + 5)))
        now = time.time()
        for kind, value in lines[start:]:
            if y + (line_height if kind == "row" else 1) > bottom:
                break
            if kind == "group":
                self.text(y, 2, GROUP_LABEL.get(value, "Other").upper(), curses.A_DIM | curses.A_BOLD)
                y += 1
                continue
            row = rows[value]
            selected = value == self.inbox_selection
            color = STATUS_COLOR.get(row["status"], 0)
            glyph = STATUS_GLYPH.get(row["status"], "?")
            if row.get("setup"):
                glyph, color = "!", WARN
            elif row.get("unverified"):
                glyph = "?"
            self.text(y, 2, glyph, curses.color_pair(color) | curses.A_BOLD if color else curses.A_DIM)
            note = ellipsis("· " + row["note"], min(36, max(0, title_width - 16))) if row.get("note") and not two_line else ""
            title = ellipsis(row["title"], title_width - (width_of(note) + 2 if note else 0))
            self.text(y, 4, pad(title, title_width), curses.A_REVERSE if selected else curses.A_BOLD if row["status"] in ("blocked", "done") else 0)
            if note:
                self.text(y, 4 + width_of(title) + 2, note, curses.color_pair(WARN) if row.get("setup") else curses.A_DIM)
            where = row["project"] + (" › " + row["branch"] if row["branch"] else "")
            who = row["machine"].label + " · " + row["harness"].capitalize()
            seen = self.activity.get(row["machine"].id + ":" + row["pane_id"], {}).get("at")
            when = age(now - seen) if seen else ""
            if two_line:
                detail = where + "   " + who + ("   " + when if when else "")
                if row.get("note"):
                    detail = row["note"] + " · " + detail
                self.text(y + 1, 4, ellipsis(detail, width - 6), curses.color_pair(WARN) if row.get("setup") else curses.A_DIM)
            else:
                cx = 4 + title_width + 2
                self.text(y, cx, ellipsis(where, 24), curses.A_DIM if not selected else 0, 24)
                self.text(y, cx + 26, ellipsis(who, 22), curses.A_DIM, 22)
                self.text(y, cx + 50, when.rjust(4), curses.A_DIM, 5)
            self.hits.extend((line, 2, width - 2, ("thread", value)) for line in range(y, y + line_height))
            y += line_height
        if not rows:
            self.text(top + 1, 4, "No matching threads." if self.filter.strip() else "Your agent threads appear here. Press n to start one.", curses.A_DIM)
        trouble = [m.label + ": " + (self.link_state.get(m.id, ("", ""))[1] or "connecting…") for m in self.machines if self.link_state.get(m.id, ("connecting", ""))[0] != "live" and not self.demo]
        if self.entry:
            self.draw_entry(height - 4, 2, width - 4)
        elif self.message:
            self.text(height - 3, 2, self.message.splitlines()[0], curses.color_pair(self.message_color) if self.message_color else 0, width - 4)
        elif trouble:
            self.text(height - 3, 2, ellipsis("   ".join(trouble), width - 4), curses.A_DIM)
        if self.recording:
            hints = ["Speak now   Enter stop and send   Ctrl+T stop and edit   Esc discard"]
        elif self.entry and self.entry.kind == "reply":
            hints = ["Enter send reply   Ctrl+T dictate   Esc cancel"]
        elif self.entry and self.entry.kind == "filter":
            hints = ["Type to filter   Enter keep filter   Esc clear"]
        else:
            hints = ["↑↓ move   Enter open   r reply   e edit and relaunch   d dismiss failed   n new   / filter   F5 rescan   Esc close"]
        self.draw_footer(height, width, hints)

    def open_thread(self):
        rows = self.filtered_rows()
        if not rows:
            return
        row = rows[self.inbox_selection]
        if self.demo:
            self.notify("Preview only. No focus changed.", INFO)
            return
        self.notify("Opening " + row["title"] + "…", INFO)
        if row.get("setup"):
            self.background("focus", row["machine"], lambda: self.herdr.call(row["machine"], "workspace", "focus", row["record"]["workspace_id"]))
        else:
            self.background("focus", row["machine"], lambda: self.herdr.focus(row["machine"], row["pane_id"]))

    def begin_reply(self):
        rows = self.filtered_rows()
        if not rows:
            return
        row = rows[self.inbox_selection]
        if row.get("setup"):
            self.notify("This launch failed. Open it to inspect, or press d to dismiss.", INFO)
        elif row["status"] == "blocked":
            self.notify("This thread is waiting for your approval. Press Enter to open it.", INFO)
        else:
            self.entry = Entry("reply", "Reply to " + ellipsis(row["title"], 32) + ": ", limit=4000, placeholder="Follow-up message…")
            self.reply_target = row

    def send_reply(self):
        text = self.entry.text.strip() if self.entry else ""
        row = self.reply_target
        self.entry = None
        if not text:
            return
        if self.demo:
            self.notify("Preview only. No reply sent.", INFO)
            return
        self.notify("Sending reply to " + row["title"] + "…", INFO)
        self.background("reply", row["machine"], lambda: self.herdr.prompt(row["machine"], row["pane_id"], text, timeout_ms=8000), tag=row["title"])

    def relaunch(self):
        """Prefill the composer from the selected thread's launch record."""
        rows = self.filtered_rows()
        if not rows:
            return
        row = rows[self.inbox_selection]
        record = row["record"]
        if not record.get("task"):
            self.notify("This thread was not started from the inbox, so its task is unknown.", INFO)
            return
        self.project = record["project"]
        self.preferred_machine, self.preferred_harness = record["machine_id"], record["harness"]
        self.preset_intent = ""
        self.restore_choice()
        self.harness = record["harness"]
        self.restore_model()
        self.model, self.thinking = record.get("model", ""), record.get("thinking", "")
        self.model_selections[self.model_context] = self.model, self.thinking
        mode = record.get("workspace", "checkout")
        self.workspace = {"mode": mode, "path": record.get("repo", "") if mode == "worktree" else record.get("cwd", ""), "branch": record.get("branch", "") if mode == "worktree" and row.get("setup") else ""}
        self.task, self.cursor, self.history_index = record["task"], len(record["task"]), None
        self.view, self.field = "new", TASK
        self.notify("Composer filled from " + ellipsis(record["title"], 40) + ". Adjust anything, then send.", INFO)

    def dismiss_setup(self):
        rows = self.filtered_rows()
        if not rows:
            return
        row = rows[self.inbox_selection]
        if not row.get("setup"):
            self.notify("Only failed launches can be dismissed. Close finished threads in Herdr.", INFO)
            return
        self.store.forget_thread(row["record"]["id"])
        self.rebuild_rows()
        self.notify("Dismissed " + row["title"], OK)

    # ----- input -----------------------------------------------------------

    def mouse(self):
        try:
            _, x, y, _, state = curses.getmouse()
        except curses.error:
            return
        if state & curses.REPORT_MOUSE_POSITION or not state & (curses.BUTTON1_CLICKED | curses.BUTTON1_PRESSED):
            return
        self.dirty = True
        for hit_y, start, end, action in self.hits:
            if y == hit_y and start <= x < end:
                if action[0] == "field":
                    self.entry = None
                    self.field = action[1]
                    if self.field in PICKER_FIELDS:
                        self.open_picker(self.field)
                    elif self.field == TASK:
                        cells = [(column, index) for row, column, index in self.task_cells if row == y]
                        self.cursor = min(cells, key=lambda cell: abs(cell[0] - x))[1] if cells else len(self.task)
                elif action[0] == "pick":
                    self.selection = action[1]
                    self.choose()
                elif action[0] == "new":
                    self.view, self.field = "new", TASK
                elif action[0] == "launch":
                    self.start()
                elif action[0] == "save":
                    self.save_preset() if self.entry and self.entry.kind == "name" else self.begin_save()
                elif action[0] == "thread":
                    self.inbox_selection = action[1]
                    self.open_thread()
                return
        if self.picker:
            self.picker = None

    def key(self, key):
        if key == curses.KEY_MOUSE:
            self.mouse()
        elif self.recording:
            if key in ENTER_KEYS or key == CTRL_ENTER:
                self.stop_dictation(send=True)
            elif key in DICTATE_KEYS:
                self.stop_dictation(send=False)
            elif key == "\x1b":
                self.cancel_dictation()
        elif key in DICTATE_KEYS:
            self.toggle_dictation()
        elif key == curses.KEY_F10:
            self.open_dictation_menu()
        elif key == "\x1b":
            if self.entry:
                kind = self.entry.kind
                self.entry = None
                if kind == "filter":
                    self.filter = ""
                if self.view == "new":
                    self.field = TASK
            elif self.picker:
                self.picker = None
            elif self.view == "inbox" and self.filter:
                self.filter = ""
            else:
                self.finished = True
        elif key == curses.KEY_F5:
            self.notify("Refreshing projects and models…", INFO)
            importlib.reload(banner)
            self.init_banner_colors()
            self.refresh(force_models=True)
        elif self.entry:
            self.entry_key(key)
        elif self.view == "new" and key in SELECTOR_KEYS:
            self.open_picker(SELECTOR_KEYS[key])
        elif self.view == "new" and key == "\x04":
            self.begin_save()
        elif self.view == "new" and key == "\x13":
            self.picker = None
            self.start()
        elif self.view == "new" and key == CTRL_ENTER:
            self.picker = None
            self.start(keep_draft=True)
        elif self.view == "new" and key in ("\t", curses.KEY_BTAB):
            self.move_field(-1 if key == curses.KEY_BTAB else 1)
        elif self.picker:
            self.picker_key(key)
        elif self.view == "inbox":
            self.inbox_key(key)
        elif self.field in PICKER_FIELDS and key in ENTER_KEYS + (" ", curses.KEY_LEFT, curses.KEY_RIGHT, curses.KEY_UP, curses.KEY_DOWN):
            self.open_picker(self.field)
        elif self.field == SEND and key in ENTER_KEYS:
            self.start()
        elif self.field == SAVE and key in ENTER_KEYS + (" ",):
            self.begin_save()
        elif self.field == TASK:
            if key in ENTER_KEYS:
                self.start()
            elif key == "\x10":
                self.recall_history(1)
            elif key == "\x0e":
                self.recall_history(-1)
            else:
                self.edit_task(key)

    def entry_key(self, key):
        entry = self.entry
        if key in ENTER_KEYS + ("\x13",):
            if entry.kind == "secret":
                self.submit_secret()
            elif entry.kind == "command":
                command = entry.text.strip()
                self.entry = None
                if command:
                    self.save_speech({"backend": "command", "command": command})
                    self.notify("Dictation uses: " + command, OK)
            elif entry.kind == "name":
                self.save_preset()
            elif entry.kind == "branch":
                self.workspace = {"mode": "worktree", "path": self.workspace.get("path", ""), "branch": entry.text.strip()}
                self.entry, self.field = None, TASK
            elif entry.kind == "reply":
                self.send_reply()
            elif entry.kind == "filter":
                self.entry = None
        elif key == "\t" and entry.kind in ("name", "branch"):
            self.entry = None
            self.move_field(1)
        else:
            entry.key(key)
            if entry.kind == "filter":
                self.filter = entry.text
                self.inbox_selection = 0

    def picker_key(self, key):
        if key == curses.KEY_UP:
            self.selection -= 1
        elif key == curses.KEY_DOWN:
            self.selection += 1
        elif key == curses.KEY_NPAGE:
            self.selection += 8
        elif key == curses.KEY_PPAGE:
            self.selection -= 8
        elif key in ENTER_KEYS:
            self.choose()
        elif self.picker == "preset" and key in ("\x12", curses.KEY_DC):
            self.manage_preset(key == "\x12")
        else:
            if key in BACKSPACE_KEYS:
                self.query = self.query[:-1]
            elif key == "\x15":
                self.query = ""
            elif isinstance(key, str) and key.isprintable():
                self.query += key
            self.selection = 0

    def inbox_key(self, key):
        if key in ("n", "N"):
            self.view, self.field = "new", TASK
        elif key == curses.KEY_UP or key == "k":
            self.inbox_selection -= 1
        elif key == curses.KEY_DOWN or key == "j":
            self.inbox_selection += 1
        elif key == curses.KEY_HOME:
            self.inbox_selection = 0
        elif key == curses.KEY_END:
            self.inbox_selection = len(self.filtered_rows())
        elif key in ENTER_KEYS or key == "o":
            self.open_thread()
        elif key == "r":
            self.begin_reply()
        elif key == "d":
            self.dismiss_setup()
        elif key == "e":
            self.relaunch()
        elif key == "/":
            self.entry = Entry("filter", "Filter: ", self.filter, limit=80, placeholder="title, project, branch, harness…")

    def edit_task(self, key):
        if key not in (curses.KEY_LEFT, curses.KEY_RIGHT, curses.KEY_UP, curses.KEY_DOWN, curses.KEY_HOME, curses.KEY_END, "\x01", "\x05"):
            self.history_index = None
        if key in BACKSPACE_KEYS:
            if self.cursor:
                self.task = self.task[:self.cursor - 1] + self.task[self.cursor:]
                self.cursor -= 1
        elif key == curses.KEY_DC:
            self.task = self.task[:self.cursor] + self.task[self.cursor + 1:]
        elif key == curses.KEY_LEFT:
            self.cursor = max(0, self.cursor - 1)
        elif key == curses.KEY_RIGHT:
            self.cursor = min(len(self.task), self.cursor + 1)
        elif key in (curses.KEY_UP, curses.KEY_DOWN):
            _, (row, column), positions = task_layout(self.task, self.cursor, max(1, self.task_width))
            target = row + (1 if key == curses.KEY_DOWN else -1)
            candidates = [(abs(col - column), index) for index, (line, col) in enumerate(positions) if line == target]
            if candidates:
                self.cursor = min(candidates)[1]
        elif key in (curses.KEY_HOME, "\x01"):
            self.cursor = 0
        elif key in (curses.KEY_END, "\x05"):
            self.cursor = len(self.task)
        elif key == "\x15":
            self.task, self.cursor = "", 0
        elif key == "\x17":
            start = self.cursor
            while start and self.task[start - 1] in " \n":
                start -= 1
            while start and self.task[start - 1] not in " \n":
                start -= 1
            self.task, self.cursor = self.task[:start] + self.task[self.cursor:], start
        elif isinstance(key, str) and (key.isprintable() or key in ("\n", "\r")):
            if len(self.task) < TASK_LIMIT:
                key = "\n" if key == "\r" else key
                self.task = self.task[:self.cursor] + key + self.task[self.cursor:]
                self.cursor += len(key)

    def paste(self, pasted):
        pasted = pasted.replace("\r\n", "\n").replace("\r", "\n").replace("\t", "    ")
        pasted = "".join(c for c in pasted if c.isprintable() or c == "\n")
        if self.entry:
            self.entry.insert(pasted)
            if self.entry.kind == "filter":
                self.filter = self.entry.text
        elif self.picker:
            self.query += pasted.replace("\n", " ")
            self.selection = 0
        elif self.view == "new" and self.field == TASK:
            pasted = pasted[:max(0, TASK_LIMIT - len(self.task))]
            self.task = self.task[:self.cursor] + pasted + self.task[self.cursor:]
            self.cursor += len(pasted)
            self.history_index = None

    # ----- terminal loop ---------------------------------------------------

    def init_banner_colors(self):
        for index, color in enumerate(banner.COLORS, BANNER_PAIR):
            fallback = curses.COLOR_CYAN if index >= 11 else curses.COLOR_YELLOW
            curses.init_pair(index, color if curses.COLORS >= 256 else fallback, -1)

    def init_colors(self):
        curses.start_color()
        curses.use_default_colors()
        for index, (color, fallback) in {ACCENT: (208, curses.COLOR_YELLOW), OK: (114, curses.COLOR_GREEN), WARN: (203, curses.COLOR_RED), INFO: (75, curses.COLOR_CYAN)}.items():
            curses.init_pair(index, color if curses.COLORS >= 256 else fallback, -1)
        self.init_banner_colors()

    def read_sequence(self):
        """Decode an escape sequence into a curses key, a paste marker, or nothing."""
        self.screen.timeout(30)
        sequence = ""
        for _ in range(12):
            try:
                character = self.screen.get_wch()
            except curses.error:
                break
            if isinstance(character, str):
                if character == "\x1b":
                    curses.unget_wch(character)
                    break
                if not sequence and character not in ("[", "O", "\r", "\n"):
                    curses.unget_wch(character)
                    break
                sequence += character
                if character in ("\r", "\n", "~") or (len(sequence) > 1 and character.isalpha()):
                    break
        self.screen.timeout(50)
        if sequence == "[200~":
            return "paste"
        if sequence in ("\r", "\n", "[13;2u", "[13;3u", "[27;2;13~", "[27;3;13~"):
            return "newline"
        if sequence in ("[13;5u", "[27;5;13~"):
            return CTRL_ENTER
        sequences = {"[A": curses.KEY_UP, "OA": curses.KEY_UP, "[B": curses.KEY_DOWN, "OB": curses.KEY_DOWN, "[C": curses.KEY_RIGHT, "OC": curses.KEY_RIGHT, "[D": curses.KEY_LEFT, "OD": curses.KEY_LEFT, "[Z": curses.KEY_BTAB, "[H": curses.KEY_HOME, "OH": curses.KEY_HOME, "[1~": curses.KEY_HOME, "[7~": curses.KEY_HOME, "[F": curses.KEY_END, "OF": curses.KEY_END, "[4~": curses.KEY_END, "[8~": curses.KEY_END, "[3~": curses.KEY_DC, "[5~": curses.KEY_PPAGE, "[6~": curses.KEY_NPAGE, "OQ": curses.KEY_F2, "OR": curses.KEY_F3, "OS": curses.KEY_F4, "[12~": curses.KEY_F2, "[13~": curses.KEY_F3, "[14~": curses.KEY_F4, "[15~": curses.KEY_F5, "[17~": curses.KEY_F6, "[18~": curses.KEY_F7, "[19~": curses.KEY_F8, "[20~": curses.KEY_F9, "[21~": curses.KEY_F10}
        if sequence in sequences:
            return sequences[sequence]
        extended = re.fullmatch(r"\[(\d+)(?:;(\d+))?u", sequence)
        if extended:
            code, modifier = int(extended.group(1)), int(extended.group(2) or 1)
            if code == 13:
                return "newline" if modifier in (2, 3) else CTRL_ENTER if modifier == 5 else "\n"
            if code == 9 and modifier == 2:
                return curses.KEY_BTAB
            if modifier == 5 and 32 <= code < 127:
                return chr(code & 31)
            return chr(code) if code < 128 else None
        return "\x1b" if not sequence else None

    def run(self):
        curses.raw()
        curses.curs_set(0)
        self.init_colors()
        curses.mousemask(curses.ALL_MOUSE_EVENTS)
        curses.set_escdelay(30)
        self.screen.timeout(50)
        # A pasted task must never activate fields or launch via embedded control keys.
        print("\x1b[=0u\x1b[?2004h", end="", flush=True)
        try:
            while not self.finished:
                self.collect()
                size = self.screen.getmaxyx()
                now = time.monotonic()
                elapsed = now - self.banner_updated
                if elapsed >= 1 / banner.FPS:
                    self.banner_updated = now
                    if self.banner_visible and not self.picker and not self.entry and not self.pasting:
                        self.banner_elapsed += min(elapsed, 0.25)
                        self.banner_pose, self.dirty = banner.pose(self.banner_elapsed), True
                if (self.recording or self.in_flight("install", self.machines[0]) or self.launches and any(note["state"] == "running" for note in self.launches)) and now - getattr(self, "_tick", 0) > 0.5:
                    self._tick, self.dirty = now, True
                if self.dirty or size != self.size:
                    self.draw()
                    self.dirty, self.size = False, size
                try:
                    key = self.screen.get_wch()
                except curses.error:
                    continue
                if self.pasting:
                    if isinstance(key, str):
                        self.paste_buffer += key
                    if self.paste_buffer.endswith("\x1b[201~"):
                        self.paste(self.paste_buffer[:-6])
                        self.pasting, self.paste_buffer, self.dirty = False, "", True
                    continue
                if key == "\x1b":
                    key = self.read_sequence()
                    if key == "paste":
                        self.pasting = True
                        continue
                    if key == "newline":
                        if self.view == "new" and self.field == TASK and not self.picker and not self.entry:
                            self.edit_task("\n")
                            self.dirty = True
                        continue
                    if key is None:
                        continue
                if key == "\x03":
                    break
                self.key(key)
                if key != curses.KEY_MOUSE:
                    self.dirty = True
        finally:
            print("\x1b[?2004l", end="", flush=True)
            if self.recording:
                self.recording.cancel()
            if self.links is not None:
                self.links.stop()
            self.pool.shutdown(wait=False, cancel_futures=True)


class DemoRecording:
    """Stands in for the microphone in previews and tests."""

    def __init__(self):
        self.started = time.monotonic()

    def elapsed(self):
        return time.monotonic() - self.started

    def stop(self):
        return os.devnull

    def cancel(self):
        pass


def demo_machines():
    from .herdr import Machine, local_machine
    return [local_machine(), Machine("studio", "Mac Studio", "me@studio"), Machine("macbook", "MacBook", "me@macbook")]


def demo_inventory(machine):
    projects = []
    for name, branches in (("cockpit", ["main", "feat/login", "fix/sentry"]), ("veezu-server", ["main"]), ("vibe-tshirt", ["main", "feat/annotations"])):
        checkouts = [{"path": "/code/" + name, "branch": branches[0], "linked": False}] + [{"path": "/code/.worktrees/" + name + "-" + branch.split("/")[-1], "branch": branch, "linked": True} for branch in branches[1:]]
        projects.append({"name": name, "path": "/code/" + name, "branch": branches[0], "checkouts": checkouts})
    models = {kind: {"selectable": True, "session_api": kind == "opencode", "default": "", "thinking_flag": "--thinking" if kind == "pi" else "--effort" if kind == "claude" else "", "thinking": ["off", "minimal", "low", "medium", "high", "xhigh", "max"] if kind == "pi" else ["low", "medium", "high", "xhigh", "max"] if kind == "claude" else [], "choices": [{"id": model, "label": model.split("/")[-1].capitalize() if kind == "claude" else model} for model in models]} for kind, models in (("codex", ["gpt-6-sol", "gpt-6-luna"]), ("claude", ["opus", "sonnet", "haiku", "fable"]), ("opencode", ["openai/gpt-6-sol", "openai/gpt-6-luna", "opencode/big-pickle"]), ("pi", ["openai-codex/gpt-6.1-sol"]))}
    return {"projects": projects, "harnesses": ["codex", "claude", "opencode", "pi"], "models": models, "models_at": time.time()}


def demo_rows(machines):
    samples = [("Fix login redirect", "cockpit", "feat/login", "codex", "blocked"), ("Add invoice export", "veezu-server", "main", "claude", "working"), ("Review navigation", "vibe-tshirt", "feat/annotations", "codex", "done"), ("Tighten CSP headers", "cockpit", "main", "pi", "idle")]
    rows = [{"machine": machines[index % len(machines)], "pane_id": "preview" + str(index), "title": title, "live_title": "", "project": project, "branch": branch, "worktree": "/" in branch, "harness": harness, "status": status, "sequence": index, "record": {}, "unverified": False} for index, (title, project, branch, harness, status) in enumerate(samples)]
    return sort_rows(rows)
