"""User configuration and separate, atomic launch journals."""

import json
import os
import tempfile
import time
from dataclasses import asdict
from pathlib import Path

from .presets import load_presets

HISTORY_LIMIT = 50


def config_dir():
    return Path(os.environ.get("HERDR_PLUGIN_CONFIG_DIR", str(Path.home() / ".config/herdr/plugins/config/lucas.herdr-inbox")))


def state_dir():
    default = Path.home() / ".local/state/herdr/plugins/lucas.herdr-inbox"
    return Path(os.environ.get("HERDR_PLUGIN_STATE_DIR", str(default)))


def read_json(path, default):
    try:
        return json.loads(path.read_text())
    except (FileNotFoundError, ValueError):
        return default


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=".inbox-", dir=str(path.parent))
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream, ensure_ascii=False, indent=2)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


DEFAULT_CONFIG = {"roots": ["~/Work", "~/Projects", "~/goinfre"], "depth": 2, "machines": {}, "projects": []}
WORKSPACE_MODES = ("checkout", "worktree")


class Store:
    def __init__(self):
        self.config = read_json(config_dir() / "config.json", DEFAULT_CONFIG.copy())
        if not isinstance(self.config, dict):
            raise ValueError("config.json must contain an object")
        self.refresh_presets()
        self.preferences = read_json(state_dir() / "preferences.json", {})

    def machine_config(self, machine):
        overrides = self.config.get("machines", {})
        return overrides.get(machine.id, overrides.get(machine.label, {}))

    def default_workspace_mode(self):
        mode = self.config.get("default_workspace", "checkout")
        if mode not in WORKSPACE_MODES:
            raise ValueError("default_workspace must be checkout or worktree")
        return mode

    def refresh_presets(self):
        values = read_json(state_dir() / "presets.json", self.config.get("presets", []))
        self.presets = load_presets({"presets": values})

    def save_preset(self, preset, original_name=None):
        self.refresh_presets()
        if original_name and original_name != preset.name and any(p.name == preset.name for p in self.presets):
            raise ValueError("A preset already has this name. Choose another name.")
        values = [asdict(p) for p in self.presets if p.name not in (preset.name, original_name)]
        values.append(asdict(preset))
        presets = load_presets({"presets": values})
        write_json(state_dir() / "presets.json", values)
        self.presets = presets

    def delete_preset(self, name):
        self.refresh_presets()
        values = [asdict(p) for p in self.presets if p.name != name]
        write_json(state_dir() / "presets.json", values)
        self.presets = load_presets({"presets": values})

    def cached_inventory(self, machine):
        return read_json(state_dir() / "inventory" / (machine.id + ".json"), None)

    def save_inventory(self, machine, inventory):
        write_json(state_dir() / "inventory" / (machine.id + ".json"), inventory)

    def remember(self, project, machine, harness, model="", thinking="", workspace_mode=None):
        # Merge other projects' choices when another launcher saved while we were open.
        self.preferences = read_json(state_dir() / "preferences.json", {})
        preference = self.preferences.get(project, {})
        preference.update(machine=machine.id, harness=harness)
        if workspace_mode in WORKSPACE_MODES:
            preference["workspace"] = workspace_mode
        preference.setdefault("models", {}).setdefault(machine.id, {})[harness] = model
        preference.setdefault("thinking", {}).setdefault(machine.id, {})[harness] = thinking
        self.preferences[project] = preference
        self.preferences["last_project"] = project
        write_json(state_dir() / "preferences.json", self.preferences)

    def remembered_model(self, project, machine, harness):
        return self.preferences.get(project, {}).get("models", {}).get(machine.id, {}).get(harness, "")

    def remembered_thinking(self, project, machine, harness):
        return self.preferences.get(project, {}).get("thinking", {}).get(machine.id, {}).get(harness, "")

    def remembered_workspace(self, project):
        mode = self.preferences.get(project, {}).get("workspace", "")
        return mode if mode in WORKSPACE_MODES else self.default_workspace_mode()

    def journal(self, record):
        write_json(state_dir() / "threads" / (record["id"] + ".json"), record)

    def forget_thread(self, identifier):
        try:
            (state_dir() / "threads" / (identifier + ".json")).unlink()
        except FileNotFoundError:
            pass

    def threads(self):
        directory = state_dir() / "threads"
        return [record for record in (read_json(path, {}) for path in sorted(directory.glob("*.json"))) if record.get("id")]

    def history(self):
        values = read_json(state_dir() / "history.json", [])
        return [value for value in values if isinstance(value, str)] if isinstance(values, list) else []

    def push_history(self, task):
        task = task.strip()
        if not task:
            return
        values = [value for value in self.history() if value != task]
        values.insert(0, task)
        write_json(state_dir() / "history.json", values[:HISTORY_LIMIT])

    def credentials(self):
        values = read_json(state_dir() / "credentials.json", {})
        return values if isinstance(values, dict) else {}

    def save_credentials(self, credentials):
        path = state_dir() / "credentials.json"
        write_json(path, credentials)
        os.chmod(path, 0o600)

    def activity(self):
        values = read_json(state_dir() / "activity.json", {})
        return values if isinstance(values, dict) else {}

    def save_activity(self, activity):
        # Keep the file small: drop entries older than a week.
        horizon = time.time() - 7 * 86400
        write_json(state_dir() / "activity.json", {key: value for key, value in activity.items() if value.get("at", 0) >= horizon})
