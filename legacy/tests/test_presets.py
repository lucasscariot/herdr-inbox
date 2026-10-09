import os
import tempfile
import unittest
from unittest.mock import patch

from inbox.presets import Preset
from inbox.store import Store
from inbox.ui import HARNESS, MACHINE, MODEL, PRESET, PROJECT, THINKING, UI, WORKSPACE


class PresetTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.env = patch.dict(os.environ, {"HERDR_PLUGIN_STATE_DIR": self.directory.name + "/state", "HERDR_PLUGIN_CONFIG_DIR": self.directory.name + "/config"})
        self.env.start()
        self.store = Store()
        self.ui = None

    def tearDown(self):
        if self.ui:
            self.ui.pool.shutdown(wait=True)
        self.env.stop()
        self.directory.cleanup()

    def create_ui(self):
        self.ui = UI(None, None, self.store, "new", demo=True)
        self.ui.task, self.ui.cursor = "Keep this draft", len("Keep this draft")
        return self.ui

    def choose(self, field, value):
        self.ui.open_picker(field)
        self.ui.selection = next(index for index, choice in enumerate(self.ui.choices()) if choice[1] == value)
        self.ui.choose()

    def test_presets_start_empty_and_user_entries_survive_rename_and_delete(self):
        self.assertEqual(self.store.presets, ())
        custom = Preset("My review setup", "pi", "my-provider/my-model", "max")
        self.store.save_preset(custom)
        self.assertEqual(Store().presets, (custom,))
        renamed = Preset("Careful review", custom.harness, custom.model, custom.thinking)
        Store().save_preset(renamed, custom.name)
        self.assertEqual(Store().presets, (renamed,))
        Store().delete_preset(renamed.name)
        self.assertEqual(Store().presets, ())

    def test_rename_collision_preserves_both_combinations(self):
        first, second = Preset("Review", "pi", "one", "max"), Preset("Build", "claude", "two")
        self.store.save_preset(first)
        self.store.save_preset(second)
        with self.assertRaisesRegex(ValueError, "already has this name"):
            self.store.save_preset(Preset(second.name, first.harness, first.model, first.thinking), first.name)
        self.assertEqual(Store().presets, (first, second))

    def test_preset_before_project_survives_machine_change_and_late_discovery(self):
        preset = Preset("Deep work", "pi", "openai-codex/gpt-6.1-sol", "max")
        self.store.save_preset(preset)
        ui = self.create_ui()
        self.choose(PRESET, preset.name)
        self.choose(PROJECT, "cockpit")
        self.choose(MACHINE, "studio")
        ui.inventories["studio"]["harnesses"] = ["claude"]
        ui.restore_choice()
        self.assertEqual((ui.harness, ui.model, ui.thinking), (preset.harness, preset.model, preset.thinking))
        self.assertEqual(ui.task, "Keep this draft")
        ui.start()
        self.assertIn("Pi is not installed", ui.message)
        self.assertFalse(ui.busy)
        self.assertEqual(self.store.threads(), [])

    def test_manual_thinking_breaks_preset_match_and_restores_with_harness(self):
        self.store.save_preset(Preset("Deep work", "pi", "openai-codex/gpt-6.1-sol", "max"))
        ui = self.create_ui()
        self.choose(PROJECT, "cockpit")
        self.choose(PRESET, "Deep work")
        self.choose(THINKING, "high")
        self.assertEqual(ui.preset_name(), "")
        self.choose(HARNESS, "claude")
        self.assertEqual(ui.thinking, "")
        self.choose(HARNESS, "pi")
        self.assertEqual((ui.model, ui.thinking), ("openai-codex/gpt-6.1-sol", "high"))

    def test_save_name_input_does_not_submit_the_task_and_uses_user_model(self):
        ui = self.create_ui()
        self.choose(HARNESS, "pi")
        ui.open_picker(MODEL)
        ui.query = "my-provider/my-model"
        ui.selection = len(ui.choices()) - 1
        ui.choose()
        self.choose(THINKING, "max")
        ui.begin_save()
        for character in "My custom preset":
            ui.key(character)
        ui.key("\x13")
        self.assertEqual(Store().presets, (Preset("My custom preset", "pi", "my-provider/my-model", "max"),))
        self.assertEqual(ui.task, "Keep this draft")
        self.assertFalse(ui.busy)
        self.assertEqual(self.store.threads(), [])

    def test_renaming_inactive_preset_preserves_current_combo(self):
        self.store.save_preset(Preset("Other setup", "claude", "fable"))
        ui = self.create_ui()
        self.choose(HARNESS, "pi")
        before = ui.harness, ui.model, ui.thinking
        ui.open_picker(PRESET)
        ui.manage_preset(rename=True)
        ui.entry.text = "Renamed setup"
        ui.save_preset()
        self.assertEqual((ui.harness, ui.model, ui.thinking), before)
        self.assertEqual(Store().presets, (Preset("Renamed setup", "claude", "fable"),))

    def test_model_picker_ranks_prefix_and_subsequence_matches(self):
        ui = self.create_ui()
        self.choose(PROJECT, "cockpit")
        self.choose(HARNESS, "opencode")
        ui.open_picker(MODEL)
        ui.query = "luna"
        labels = [label for label, _, _ in ui.choices()]
        self.assertEqual(labels[0], "openai/gpt-6-luna")
        ui.query = "bgpk"
        self.assertEqual([value for _, value, _ in ui.choices()][0], "opencode/big-pickle")
        ui.query = "custom/model"
        self.assertEqual(ui.choices()[-1][1], "custom/model")

    def test_workspace_picker_offers_worktrees_and_remembers_mode_per_project(self):
        ui = self.create_ui()
        self.choose(PROJECT, "cockpit")
        self.assertEqual(ui.workspace["mode"], "checkout")
        self.assertEqual(ui.workspace_label(), "Checkout · main")
        values = [value for _, value, _ in ui.choices()] if ui.open_picker(WORKSPACE) is None else []
        self.assertEqual(values[:2], ["worktree", "worktree:named"])
        self.assertIn("checkout:/code/.worktrees/cockpit-login", values)
        self.choose(WORKSPACE, "checkout:/code/.worktrees/cockpit-login")
        self.assertEqual(ui.workspace_label(), "Worktree · feat/login")
        self.choose(WORKSPACE, "worktree")
        self.assertEqual(ui.workspace_label(), "New worktree")
        self.choose(WORKSPACE, "worktree:named")
        self.assertEqual(ui.entry.kind, "branch")
        for character in "feat/fast-login":
            ui.key(character)
        ui.key("\n")
        self.assertEqual(ui.workspace, {"mode": "worktree", "path": "/code/cockpit", "branch": "feat/fast-login"})
        self.assertEqual(ui.workspace_label(), "New worktree · feat/fast-login")
        self.assertEqual(ui.task, "Keep this draft")
        self.store.remember("cockpit", ui.machine, "codex", workspace_mode="worktree")
        self.choose(PROJECT, "veezu-server")
        self.assertEqual(ui.workspace["mode"], "checkout")
        self.choose(PROJECT, "cockpit")
        self.assertEqual(ui.workspace["mode"], "worktree")

    def test_history_recall_restores_the_draft(self):
        self.store.push_history("Older task")
        self.store.push_history("Newest task")
        ui = self.create_ui()
        ui.key("\x10")
        self.assertEqual(ui.task, "Newest task")
        ui.key("\x10")
        self.assertEqual(ui.task, "Older task")
        ui.key("\x0e")
        ui.key("\x0e")
        self.assertEqual(ui.task, "Keep this draft")
        self.assertIsNone(ui.history_index)


if __name__ == "__main__":
    unittest.main()
