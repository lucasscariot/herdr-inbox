import json
import os
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from inbox.herdr import Herdr, HerdrError, Machine, host_command, run_json
from inbox.inventory import PROBE, models_fresh
from inbox.store import Store
from inbox.threads import STALLED, STARTUP_BLOCKED, branch_name, inbox_rows, launch, resolve_workspace, resumable, resume, sort_rows


class FakeHerdr(Herdr):
    def __init__(self, fail=None, stalled=False, startup_blocked=False):
        super().__init__("herdr-test")
        self.calls, self.fail, self.stalled, self.startup_blocked = [], fail, stalled, startup_blocked

    def call(self, machine, *args, **kwargs):
        self.calls.append((machine, args))
        if args[:2] == self.fail:
            raise HerdrError("Connection lost after request was sent")
        if args[:2] == ("agent", "start") and self.startup_blocked:
            raise HerdrError("agent " + args[2] + " is " + STARTUP_BLOCKED + " and is not ready for prompts")
        if args[:2] == ("agent", "prompt") and self.stalled:
            raise HerdrError("agent prompt produced " + STALLED + " within 5000 ms; current status is idle")
        if args[:2] == ("workspace", "create"):
            return {"workspace": {"workspace_id": "w7"}, "root_pane": {"pane_id": "w7:p1"}, "tab": {"tab_id": "w7:t1"}}
        if args[:2] == ("worktree", "create"):
            branch = args[args.index("--branch") + 1]
            return {"workspace": {"workspace_id": "w8", "worktree": {"checkout_path": "/home/me/.herdr/worktrees/a project/" + branch}}, "root_pane": {"pane_id": "w8:p1"}, "tab": {"tab_id": "w8:t1"}, "worktree": {"branch": branch}}
        return {}


class LaunchTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.env = patch.dict(os.environ, {"HERDR_PLUGIN_STATE_DIR": self.directory.name + "/state", "HERDR_PLUGIN_CONFIG_DIR": self.directory.name + "/config"})
        self.env.start()
        self.store = Store()
        self.machine = Machine("studio", "Mac Studio", "me@studio", "agents")
        self.project = {"name": "a project", "path": "/code/a project"}

    def tearDown(self):
        self.env.stop()
        self.directory.cleanup()

    def test_remote_launch_preserves_target_paths_and_literal_prompt(self):
        gateway = FakeHerdr()
        task = "Fix `$(touch /tmp/should-not-exist)`\nDo not interpret this as shell."
        self.store.config["harness_args"] = {"codex": ["--no-daemon"]}
        record = launch(gateway, self.store, self.machine, self.project, "codex", task)
        self.assertEqual(record["stage"], "submitted")
        self.assertTrue(all(machine == self.machine for machine, _ in gateway.calls))
        create = gateway.calls[0][1]
        self.assertIn("/code/a project", create)
        self.assertIn("--no-focus", create)
        start = next(args for _, args in gateway.calls if args[:2] == ("agent", "start"))
        self.assertEqual(start[-2:], ("--", "--no-daemon"))
        prompt = gateway.calls[-1][1]
        self.assertEqual(prompt[2:4], ("w7:p1", task))
        self.assertIn("working", prompt)

    def test_uncertain_submission_is_journaled_and_never_replayed(self):
        gateway = FakeHerdr(fail=("agent", "prompt"))
        with self.assertRaisesRegex(HerdrError, "Inspect it"):
            launch(gateway, self.store, self.machine, self.project, "codex", "Fix login")
        record, = self.store.threads()
        self.assertEqual(record["stage"], "needs_attention")
        self.assertEqual(record["failed_stage"], "submitting")
        self.assertEqual(record["pane_id"], "w7:p1")
        self.assertEqual(sum(args[:2] == ("workspace", "create") for _, args in gateway.calls), 1)
        self.assertEqual(sum(args[:2] == ("agent", "prompt") for _, args in gateway.calls), 1)

    def test_selected_model_overrides_native_config_and_is_scoped_to_host_and_harness(self):
        gateway = FakeHerdr()
        self.store.config["harness_args"] = {"codex": ["--no-daemon", "--model", "old-model"]}
        inventory = {"models": {"codex": {"selectable": True}}}
        record = launch(gateway, self.store, self.machine, self.project, "codex", "Review login", model="chosen-model", inventory=inventory)
        start = next(args for _, args in gateway.calls if args[:2] == ("agent", "start"))
        self.assertEqual(start[-4:], ("--", "--no-daemon", "--model", "chosen-model"))
        self.assertEqual(record["model"], "chosen-model")
        self.assertEqual(self.store.remembered_model("a project", self.machine, "codex"), "chosen-model")
        self.assertEqual(self.store.remembered_model("a project", self.machine, "claude"), "")
        self.assertEqual(self.store.remembered_model("a project", Machine("local", "Local"), "codex"), "")

    def test_opencode_uses_acknowledged_session_submission_even_when_terminal_input_fails(self):
        gateway = FakeHerdr(fail=("agent", "prompt"))
        inventory = {"models": {"opencode": {"selectable": True, "session_api": True}}}
        with patch("inbox.threads.opencode.create") as create, patch("inbox.threads.opencode.select_model"), patch("inbox.threads.opencode.prompt") as prompt:
            record = launch(gateway, self.store, self.machine, self.project, "opencode", "Review login", model="provider/model", inventory=inventory)
        create.assert_called_once()
        self.assertEqual(create.call_args.args[2], "provider/model")
        prompt.assert_called_once()
        self.assertEqual(prompt.call_args.args[2], "Review login")
        start = next(args for _, args in gateway.calls if args[:2] == ("agent", "start"))
        self.assertEqual(start[-3:], ("--", "--session", record["native_session_id"]))
        self.assertEqual(record["stage"], "submitted")
        self.assertFalse(any(args[:2] == ("agent", "prompt") for _, args in gateway.calls))

    def test_pi_combo_overrides_model_and_thinking_and_remembers_both(self):
        gateway = FakeHerdr()
        self.store.config["harness_args"] = {"pi": ["--tui-mode", "fullscreen", "--model=old", "--thinking", "high", "--thinking=low"]}
        inventory = {"models": {"pi": {"selectable": True, "thinking_flag": "--thinking", "thinking": ["high", "max"]}}}
        model = "openai-codex/gpt-6.1-sol"
        record = launch(gateway, self.store, self.machine, self.project, "pi", "Review login", model=model, thinking="max", inventory=inventory)
        start = next(args for _, args in gateway.calls if args[:2] == ("agent", "start"))
        self.assertEqual(start[start.index("--") + 1:], ("--tui-mode", "fullscreen", "--thinking", "max", "--model", model))
        self.assertEqual(record["thinking"], "max")
        reopened = Store()
        self.assertEqual(reopened.remembered_model(self.project["name"], self.machine, "pi"), model)
        self.assertEqual(reopened.remembered_thinking(self.project["name"], self.machine, "pi"), "max")
        self.assertEqual(reopened.remembered_thinking(self.project["name"], self.machine, "claude"), "")
        self.assertEqual(reopened.remembered_thinking(self.project["name"], Machine("local", "Local"), "pi"), "")

    def test_claude_combo_uses_native_effort_flag(self):
        gateway = FakeHerdr()
        self.store.config["harness_args"] = {"claude": ["--effort=low"]}
        inventory = {"models": {"claude": {"selectable": True, "thinking_flag": "--effort", "thinking": ["max"]}}}
        launch(gateway, self.store, self.machine, self.project, "claude", "Review login", model="fable", thinking="max", inventory=inventory)
        start = next(args for _, args in gateway.calls if args[:2] == ("agent", "start"))
        self.assertEqual(start[-5:], ("--", "--effort", "max", "--model", "fable"))

    def test_unsupported_thinking_does_not_create_a_workspace(self):
        gateway = FakeHerdr()
        inventory = {"models": {"pi": {"selectable": True, "thinking_flag": "--thinking", "thinking": ["high"]}}}
        with self.assertRaisesRegex(ValueError, "does not support thinking level max"):
            launch(gateway, self.store, self.machine, self.project, "pi", "Review login", model="model", thinking="max", inventory=inventory)
        self.assertEqual(gateway.calls, [])
        self.assertEqual(self.store.threads(), [])

    def test_uncertain_native_submission_is_not_replayed(self):
        inventory = {"models": {"opencode": {"selectable": True, "session_api": True}}}
        with patch("inbox.threads.opencode.create"), patch("inbox.threads.opencode.prompt", side_effect=HerdrError("Connection lost after submission")) as prompt:
            with self.assertRaisesRegex(HerdrError, "Inspect it"):
                launch(FakeHerdr(), self.store, self.machine, self.project, "opencode", "Review login", inventory=inventory)
        prompt.assert_called_once()
        record, = self.store.threads()
        self.assertEqual(record["failed_stage"], "submitting")
        self.assertIn("native_session_id", record)

    def test_model_validation_precedes_workspace_creation(self):
        gateway = FakeHerdr()
        with self.assertRaisesRegex(ValueError, "provider/model"):
            launch(gateway, self.store, self.machine, self.project, "opencode", "Review login", model="no-provider", inventory={"models": {"opencode": {"selectable": True, "session_api": True}}})
        self.assertEqual(gateway.calls, [])

    def test_remote_command_keeps_task_and_directory_literal(self):
        import shlex
        task = "Do not run `$(touch /tmp/should-not-exist)`"
        argv = host_command(self.machine, ["opencode", "api", "session.prompt", "--data", json.dumps({"text": task})], "/code/space $(ignored)")
        outer = shlex.split(argv[-1])
        self.assertEqual(outer[:2], ["sh", "-lc"])
        command = shlex.split(outer[2])
        self.assertIn("/code/space $(ignored)", command)
        self.assertEqual(command[-1], json.dumps({"text": task}))

    def test_machine_and_session_scope_duplicate_pane_ids(self):
        records = [{"pane_id": "w1:p1", "machine_id": "other", "session": "agents", "title": "Wrong thread"}, {"pane_id": "w1:p1", "machine_id": "studio", "session": "default", "title": "Wrong session"}]
        agents = [{"pane_id": "w1:p1", "cwd": "/code/current", "agent_status": "blocked", "terminal_title_stripped": "Current task"}]
        row, = inbox_rows(self.machine, agents, records)
        self.assertEqual(row["title"], "Current task")

    def test_attention_precedes_working_independent_of_host_sequence(self):
        working = {"machine": self.machine, "project": "api", "status": "working", "sequence": 90000}
        blocked = {"machine": self.machine, "project": "web", "status": "blocked", "sequence": 1}
        self.assertEqual(sort_rows([working, blocked])[0], blocked)

    def test_inventory_handles_nested_repositories_and_spaces_without_entering_dependencies(self):
        root = Path(self.directory.name) / "code"
        for path in [root / "A project" / ".git", root / "group" / "api" / ".git", root / "node_modules" / "ignored" / ".git"]:
            path.mkdir(parents=True)
        inventory = run_json(["python3", "-c", PROBE, json.dumps({"roots": [str(root)], "depth": 2, "include_models": False})])
        self.assertEqual([p["name"] for p in inventory["projects"]], ["A project", "api"])

    def test_only_documented_empty_cli_success_is_accepted(self):
        with patch("inbox.herdr.subprocess.run") as run:
            run.return_value.stdout, run.return_value.stderr, run.return_value.returncode = "", "", 0
            gateway = Herdr("herdr-test")
            gateway.call(self.machine, "pane", "report-metadata", "w1:p1")
            import subprocess
            self.assertEqual(run.call_args.kwargs["stdin"], subprocess.DEVNULL)
            self.assertTrue(run.call_args.kwargs["start_new_session"])
            with self.assertRaises(HerdrError):
                gateway.call(self.machine, "workspace", "create")

    def test_native_error_body_is_used_when_cli_stderr_only_has_http_status(self):
        with patch("inbox.herdr.subprocess.run") as run:
            run.return_value.stdout = '{"_tag":"UnknownModel","message":"Model is unavailable on this machine"}'
            run.return_value.stderr, run.return_value.returncode = "HTTP 400 Bad Request", 1
            with self.assertRaisesRegex(HerdrError, "Model is unavailable"):
                run_json(["opencode", "api", "session.create"])

    def test_catalog_handles_opencode_version_prefix_and_help_on_stderr(self):
        import sys
        directory = Path(self.directory.name) / "bin"
        directory.mkdir()
        executable = directory / "opencode"
        state = Path(self.directory.name) / "opencode/model.json"
        state.parent.mkdir()
        state.write_text(json.dumps({"recent": [{"providerID": "provider", "modelID": "model"}], "variant": {"provider/model": "high"}}))
        for version, help_text, expected_api in [("opencode v2.0.22", "--help", True), ("1.18.30", "--model MODEL", False)]:
            executable.write_text('#!/bin/sh\ncase "$1" in\n--version) printf "' + version + '\\n";;\n--help) printf -- "' + help_text + '\\n" >&2;;\nmodels) printf "provider/model\\n";;\nesac\n')
            executable.chmod(0o755)
            env = dict(os.environ, PATH=str(directory), XDG_STATE_HOME=self.directory.name)
            result = run_json([sys.executable, "-c", PROBE, '{"roots":[],"depth":0}'], env=env)
            catalog = result["models"]["opencode"]
            self.assertTrue(catalog["selectable"])
            self.assertEqual(catalog["session_api"], expected_api)
            self.assertEqual(catalog["choices"], [{"id": "provider/model", "label": "provider/model"}])
            self.assertEqual(catalog["default"], "provider/model#high")

    def test_catalog_reads_pi_thinking_levels_from_installed_help(self):
        import sys
        directory = Path(self.directory.name) / "bin"
        directory.mkdir()
        executable = directory / "pi"
        executable.write_text('#!/bin/sh\ncase "$1" in\n--help) printf -- "  --model MODEL\\n  --thinking LEVEL Set thinking level: off, high, max\\n";;\n--offline) printf "provider model context max-out thinking images\\nopenai-codex chosen 272K 128K yes yes\\n";;\nesac\n')
        executable.chmod(0o755)
        result = run_json([sys.executable, "-c", PROBE, '{"roots":[],"depth":0}'], env=dict(os.environ, PATH=str(directory)))
        catalog = result["models"]["pi"]
        self.assertEqual(catalog["thinking_flag"], "--thinking")
        self.assertEqual(catalog["thinking"], ["off", "high", "max"])

    def test_worktree_launch_names_the_branch_after_the_task_and_records_its_checkout(self):
        gateway = FakeHerdr()
        project = dict(self.project, checkouts=[{"path": "/code/a project", "branch": "main", "linked": False}, {"path": "/code/wt", "branch": "fix-login", "linked": True}])
        record = launch(gateway, self.store, self.machine, project, "codex", "Fix login so the redirect loop stops", workspace={"mode": "worktree"})
        create = next(args for _, args in gateway.calls if args[:2] == ("worktree", "create"))
        self.assertEqual(create[create.index("--cwd") + 1], "/code/a project")
        self.assertEqual(create[create.index("--branch") + 1], "fix-login-so-redirect-loop-stops")
        self.assertIn("--no-focus", create)
        self.assertNotIn("--base", create)
        self.assertFalse(any(args[:2] == ("workspace", "create") for _, args in gateway.calls))
        self.assertEqual(record["stage"], "submitted")
        self.assertEqual((record["workspace"], record["branch"]), ("worktree", "fix-login-so-redirect-loop-stops"))
        self.assertEqual(record["cwd"], "/home/me/.herdr/worktrees/a project/fix-login-so-redirect-loop-stops")
        self.assertEqual(record["pane_id"], "w8:p1")
        self.assertEqual(Store().remembered_workspace("a project"), "worktree")
        self.assertEqual(Store().history(), ["Fix login so the redirect loop stops"])

    def test_named_worktree_uses_base_and_prefix_and_rejects_bad_branches_before_mutating(self):
        gateway = FakeHerdr()
        self.store.config["branch_prefix"] = "lucas/"
        launch(gateway, self.store, self.machine, self.project, "codex", "Review", workspace={"mode": "worktree", "branch": "review-pass", "base": "origin/main"})
        create = next(args for _, args in gateway.calls if args[:2] == ("worktree", "create"))
        self.assertEqual(create[create.index("--branch") + 1], "review-pass")
        self.assertEqual(create[create.index("--base") + 1], "origin/main")
        gateway = FakeHerdr()
        launch(gateway, self.store, self.machine, self.project, "codex", "Review the diff", workspace={"mode": "worktree"})
        create = next(args for _, args in gateway.calls if args[:2] == ("worktree", "create"))
        self.assertEqual(create[create.index("--branch") + 1], "lucas/review-diff")
        gateway = FakeHerdr()
        with self.assertRaisesRegex(ValueError, "Branch names"):
            launch(gateway, self.store, self.machine, self.project, "codex", "Review", workspace={"mode": "worktree", "branch": "bad..name"})
        self.assertEqual(gateway.calls, [])
        self.assertEqual(sorted(r["branch"] for r in self.store.threads()), ["lucas/review-diff", "review-pass"])

    def test_branch_names_skip_filler_words_and_taken_names(self):
        self.assertEqual(branch_name("Please fix the login redirect loop on mobile"), "fix-login-redirect-loop-mobile")
        self.assertEqual(branch_name("Please fix the login redirect loop on mobile", taken={"fix-login-redirect-loop-mobile"}), "fix-login-redirect-loop-mobile-2")
        self.assertEqual(branch_name("   "), "thread")
        self.assertEqual(branch_name("Ship it\nwith a second line"), "ship-it")

    def test_existing_worktree_checkout_runs_in_place(self):
        gateway = FakeHerdr()
        project = dict(self.project, checkouts=[{"path": "/code/a project", "branch": "main", "linked": False}, {"path": "/code/wt", "branch": "fix-login", "linked": True}])
        record = launch(gateway, self.store, self.machine, project, "codex", "Continue", workspace={"mode": "checkout", "path": "/code/wt"})
        create = next(args for _, args in gateway.calls if args[:2] == ("workspace", "create"))
        self.assertEqual(create[create.index("--cwd") + 1], "/code/wt")
        self.assertEqual((record["cwd"], record["branch"], record["workspace"]), ("/code/wt", "fix-login", "checkout"))
        resolved = resolve_workspace(project, {"mode": "checkout", "path": "/missing"}, "task")
        self.assertEqual(resolved["path"], "/code/a project")

    def test_stalled_prompt_is_submitted_but_unverified(self):
        gateway = FakeHerdr(stalled=True)
        record = launch(gateway, self.store, self.machine, self.project, "codex", "Say hi")
        self.assertEqual(record["stage"], "submitted")
        self.assertTrue(record["unverified"])
        self.assertEqual(sum(args[:2] == ("agent", "prompt") for _, args in gateway.calls), 1)
        agents = [{"pane_id": "w7:p1", "cwd": "/code/a project", "agent_status": "idle", "state_change_seq": 3}]
        row, = inbox_rows(self.machine, agents, self.store.threads())
        self.assertTrue(row["unverified"])
        agents[0]["agent_status"] = "working"
        row, = inbox_rows(self.machine, agents, self.store.threads())
        self.assertFalse(row["unverified"])

    def test_inbox_rows_take_branches_from_inventory_and_ignore_shell_prompt_titles(self):
        inventory = {"projects": [{"name": "api", "path": "/code/api", "checkouts": [{"path": "/code/api", "branch": "main", "linked": False}, {"path": "/code/.wt/api-login", "branch": "feat/login", "linked": True}]}]}
        agents = [{"pane_id": "w1:p1", "cwd": "/code/.wt/api-login", "agent_status": "working", "agent": "claude", "terminal_title_stripped": "me@host:~/code"}, {"pane_id": "w2:p1", "cwd": "/code/api", "agent_status": "idle", "agent": "codex", "terminal_title_stripped": "Add invoice export"}]
        login, main = inbox_rows(self.machine, agents, [], inventory)
        self.assertEqual((login["project"], login["branch"], login["worktree"], login["title"]), ("api", "feat/login", True, "w1:p1"))
        self.assertEqual((main["project"], main["branch"], main["worktree"], main["title"]), ("api", "main", False, "Add invoice export"))

    def test_inventory_groups_linked_worktrees_under_their_repository(self):
        root = Path(self.directory.name) / "code"
        main = root / "api"
        (main / ".git" / "worktrees" / "login").mkdir(parents=True)
        (main / ".git" / "HEAD").write_text("ref: refs/heads/main\n")
        linked = root / "api-login"
        linked.mkdir()
        (linked / ".git").write_text("gitdir: " + str(main / ".git" / "worktrees" / "login") + "\n")
        (main / ".git" / "worktrees" / "login" / "gitdir").write_text(str(linked / ".git") + "\n")
        (main / ".git" / "worktrees" / "login" / "HEAD").write_text("ref: refs/heads/feat/login\n")
        elsewhere = Path(self.directory.name) / "elsewhere" / "api-docs"
        elsewhere.mkdir(parents=True)
        (elsewhere / ".git").write_text("gitdir: " + str(main / ".git" / "worktrees" / "docs") + "\n")
        (main / ".git" / "worktrees" / "docs").mkdir()
        (main / ".git" / "worktrees" / "docs" / "gitdir").write_text(str(elsewhere / ".git") + "\n")
        (main / ".git" / "worktrees" / "docs" / "HEAD").write_text("ref: refs/heads/docs\n")
        inventory = run_json(["python3", "-c", PROBE, json.dumps({"roots": [str(root)], "depth": 2, "include_models": False})])
        project, = inventory["projects"]
        self.assertEqual((project["name"], project["path"], project["branch"]), ("api", str(main.resolve()), "main"))
        self.assertEqual([(c["branch"], c["linked"]) for c in project["checkouts"]], [("main", False), ("docs", True), ("feat/login", True)])
        self.assertEqual(project["checkouts"][1]["path"], str(elsewhere.resolve()))

    def test_model_catalogs_are_reused_while_fresh(self):
        import time
        self.assertFalse(models_fresh(None))
        self.assertFalse(models_fresh({"models": {"codex": {}}, "models_at": time.time() - 3600}))
        self.assertFalse(models_fresh({"models": {}, "models_at": time.time()}))
        self.assertTrue(models_fresh({"models": {"codex": {}}, "models_at": time.time() - 60}))

    def test_startup_dialog_keeps_the_task_and_resumes_once_idle(self):
        gateway = FakeHerdr(startup_blocked=True)
        record = launch(gateway, self.store, self.machine, self.project, "claude", "Say pong", workspace={"mode": "worktree"})
        self.assertEqual(record["stage"], "startup_blocked")
        self.assertFalse(any(args[:2] == ("agent", "prompt") for _, args in gateway.calls))
        self.assertEqual(Store().history(), ["Say pong"])
        agents = [{"pane_id": "w8:p1", "cwd": record["cwd"], "agent_status": "blocked", "agent": "claude"}]
        row, = inbox_rows(self.machine, agents, self.store.threads())
        self.assertEqual(row["note"], "answer the startup prompt, the task follows")
        self.assertEqual(resumable([row]), [])
        agents[0]["agent_status"] = "idle"
        row, = inbox_rows(self.machine, agents, self.store.threads())
        self.assertEqual(resumable([row]), [row])
        gateway = FakeHerdr()
        resumed = resume(gateway, self.store, row["record"])
        prompt = next(args for _, args in gateway.calls if args[:2] == ("agent", "prompt"))
        self.assertEqual(prompt[2:4], ("w8:p1", "Say pong"))
        self.assertEqual(resumed["stage"], "submitted")
        self.assertEqual(Store().threads()[0]["stage"], "submitted")


if __name__ == "__main__":
    unittest.main()
