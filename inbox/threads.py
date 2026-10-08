"""Create exactly one workspace for a launch, then start and prompt its agent."""

import re
import time
import uuid
from pathlib import Path

from .herdr import HerdrError, Machine
from . import opencode
from .inventory import checkout_for, discover

STALLED = "no observed working or blocked state"
STARTUP_BLOCKED = "blocked during startup"


def task_title(task):
    return " ".join(task.strip().split())[:72]


def branch_name(task, taken=(), prefix=""):
    """Derive a short, unique Git branch name from the first line of a task."""
    first = task.strip().splitlines()[0] if task.strip() else ""
    words = re.sub(r"[^a-z0-9]+", "-", first.lower()).strip("-").split("-")
    slug = ""
    for word in words:
        if word in ("a", "an", "the", "to", "of", "in", "on", "for", "and", "or", "with", "please", "can", "you"):
            continue
        candidate = slug + ("-" if slug else "") + word
        if len(candidate) > 32 and slug:
            break
        slug = candidate
    slug = (slug or "thread")[:40].strip("-")
    name = prefix + slug
    suffix = 2
    while name in taken:
        name = prefix + slug + "-" + str(suffix)
        suffix += 1
    return name


def resolve_workspace(project, workspace, task, prefix=""):
    """Normalize the workspace choice into a checkout path or a worktree request."""
    workspace = dict(workspace or {})
    mode = workspace.get("mode", "checkout")
    if mode == "worktree":
        taken = {c["branch"] for c in project.get("checkouts", [])}
        branch = (workspace.get("branch") or "").strip() or branch_name(task, taken, prefix)
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._/-]*", branch) or branch.endswith((".lock", "/")) or ".." in branch or "//" in branch or "@{" in branch:
            raise ValueError("Branch names use letters, digits, dots, dashes, and slashes: " + branch)
        if branch in taken and not workspace.get("existing"):
            raise ValueError("Branch " + branch + " already has a worktree. Choose it from the Workspace list instead.")
        return {"mode": "worktree", "branch": branch, "base": (workspace.get("base") or "").strip()}
    if mode != "checkout":
        raise ValueError("Workspace mode must be checkout or worktree")
    checkout = checkout_for(project, workspace.get("path") or project["path"])
    return {"mode": "checkout", "path": checkout["path"], "branch": checkout.get("branch", ""), "linked": checkout.get("linked", False)}


def launch(herdr, store, machine, project, harness, task, progress=lambda message: None, model="", inventory=None, thinking="", workspace=None):
    task = task.strip()
    if not task:
        raise ValueError("Write a task before launching.")
    extra_args = store.machine_config(machine).get("harness_args", store.config.get("harness_args", {})).get(harness, [])
    if not isinstance(extra_args, list) or not all(isinstance(arg, str) for arg in extra_args):
        raise ValueError("harness_args must contain lists of argument strings")
    prefix = store.machine_config(machine).get("branch_prefix", store.config.get("branch_prefix", ""))
    if not isinstance(prefix, str):
        raise ValueError("branch_prefix must be a string")
    workspace = resolve_workspace(project, workspace, task, prefix)
    if harness == "opencode" or model or thinking:
        inventory = inventory or store.cached_inventory(machine)
        if not inventory or harness not in inventory.get("models", {}) or thinking and "thinking" not in inventory["models"][harness]:
            inventory = discover(machine, store)
    catalog = (inventory or {}).get("models", {}).get(harness, {})
    session_api = harness == "opencode" and catalog.get("session_api", False)
    configured_model = ""
    if model or session_api:
        # Native flags and the UI must not compete for the same model setting.
        configured, skip = [], False
        for argument in extra_args:
            if skip:
                configured_model, skip = argument, False
            elif argument in ("--model", "-m"):
                skip = True
            elif argument.startswith("--model="):
                configured_model = argument.split("=", 1)[1]
            else:
                configured.append(argument)
        extra_args = configured
    effective_model = (model or configured_model or catalog.get("default", "")) if session_api else model
    if model and not catalog.get("selectable", False):
        raise ValueError("This installed harness does not support choosing a model at launch")
    if thinking:
        flag = catalog.get("thinking_flag")
        if flag not in ("--thinking", "--effort") or thinking not in catalog.get("thinking", []):
            raise ValueError("This installed harness does not support thinking level " + thinking)
        configured, skip = [], False
        for argument in extra_args:
            if skip:
                skip = False
            elif argument == flag:
                skip = True
            elif not argument.startswith(flag + "="):
                configured.append(argument)
        extra_args = configured + [flag, thinking]
    if session_api:
        if effective_model and ("/" not in effective_model or not all(effective_model.split("/", 1))):
            raise ValueError("OpenCode models use provider/model IDs")
        if any(a in ("--standalone", "--session", "-s", "--continue", "-c", "--prompt") or a.startswith(("--session=", "--prompt=")) for a in extra_args):
            raise ValueError("OpenCode thread creation owns its session and prompt; remove those overrides from harness_args")
    elif model:
        extra_args += ["--model", model]
    title = task_title(task)
    identifier = uuid.uuid4().hex
    slug = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")[:16] or "thread"
    record = {"id": identifier, "machine_id": machine.id, "machine_label": machine.label, "session": machine.session, "project": project["name"], "repo": project["path"], "harness": harness, "model": model, "thinking": thinking, "title": title, "task": task, "agent_name": "t-" + slug + "-" + identifier[:8], "created_at": time.time(), "stage": "creating", "workspace": workspace["mode"], "branch": workspace.get("branch", "")}
    record["cwd"] = workspace["path"] if workspace["mode"] == "checkout" else ""
    if session_api:
        record["effective_model"] = effective_model
    # Write intent before the first mutation. Each launch owns its own journal.
    store.journal(record)
    try:
        if workspace["mode"] == "worktree":
            progress("Creating worktree " + workspace["branch"] + " on " + machine.label + "...")
            created = herdr.create_worktree(machine, project["path"], workspace["branch"], project["name"], workspace.get("base", ""))
            record["cwd"] = created.get("workspace", {}).get("worktree", {}).get("checkout_path") or created.get("worktree", {}).get("path") or created["root_pane"].get("cwd", "")
        else:
            progress("Creating thread on " + machine.label + "...")
            created = herdr.create_workspace(machine, workspace["path"], project["name"])
        record.update(workspace_id=created["workspace"]["workspace_id"], pane_id=created["root_pane"]["pane_id"], tab_id=created["tab"]["tab_id"], stage="created")
        store.journal(record)
        herdr.call(machine, "tab", "rename", record["tab_id"], title)
        if session_api:
            record.update(native_session_id="ses_" + identifier, stage="creating_session")
            store.journal(record)
            opencode.create(machine, record, effective_model, extra_args)
            extra_args = extra_args + ["--session", record["native_session_id"]]
        progress("Starting " + harness + "...")
        record["stage"] = "starting"
        store.journal(record)
        start_args = ["agent", "start", record["agent_name"], "--kind", harness, "--pane", record["pane_id"], "--timeout", "45000"]
        if extra_args:
            start_args += ["--"] + extra_args
        try:
            herdr.call(machine, *start_args, timeout=60)
        except HerdrError as error:
            if STARTUP_BLOCKED not in str(error).lower() or session_api:
                raise
            # A trust, login, or update dialog is waiting in the terminal. Keep the
            # task; the inbox sends it as soon as the agent becomes idle.
            record["stage"] = "startup_blocked"
            store.journal(record)
            herdr.call(machine, "pane", "report-metadata", record["pane_id"], "--source", "plugin:lucasscariot.herdr-inbox", "--display-agent", harness.capitalize(), "--token", "thread=" + title)
            store.remember(project["name"], machine, harness, model, thinking, workspace["mode"])
            store.push_history(task)
            progress("Waiting for the startup prompt in the thread")
            return record
        record["stage"] = "ready"
        store.journal(record)
        herdr.call(machine, "pane", "report-metadata", record["pane_id"], "--source", "plugin:lucasscariot.herdr-inbox", "--display-agent", harness.capitalize(), "--token", "thread=" + title)
        if session_api and effective_model:
            record["stage"] = "selecting_model"
            store.journal(record)
            opencode.select_model(machine, record, effective_model, extra_args)
        progress("Sending task...")
        record["stage"] = "submitting"
        store.journal(record)
        if session_api:
            opencode.prompt(machine, record, task, extra_args)
        else:
            try:
                herdr.prompt(machine, record["pane_id"], task)
            except HerdrError as error:
                # Herdr accepted the keystrokes but saw no state change in time. A fast
                # answer or a slow first turn both look like this; the inbox shows it.
                if STALLED not in str(error):
                    raise
                record["unverified"] = True
        record["stage"] = "submitted"
        store.journal(record)
        store.remember(project["name"], machine, harness, model, thinking, workspace["mode"])
        store.push_history(task)
        return record
    except (HerdrError, OSError, KeyError, ValueError) as error:
        previous_stage = record["stage"]
        record.update(stage="needs_attention", failed_stage=previous_stage, error=str(error))
        store.journal(record)
        location = "Pane " + record["pane_id"] if record.get("pane_id") else "The create request may have reached the server"
        raise HerdrError(str(error) + "\n" + location + " on " + machine.label + ". Inspect it in the sidebar before launching again.") from error


def resume(herdr, store, record):
    """Send the task of a launch that stopped at a startup dialog, once the agent is idle."""
    machine = Machine(record["machine_id"], record["machine_label"], session=record.get("session", "default"))
    record = dict(record, stage="submitting")
    store.journal(record)
    try:
        herdr.prompt(machine, record["pane_id"], record["task"])
    except HerdrError as error:
        if STALLED not in str(error):
            record.update(stage="needs_attention", failed_stage="submitting", error=str(error))
            store.journal(record)
            raise
        record["unverified"] = True
    record["stage"] = "submitted"
    store.journal(record)
    return record


def resumable(rows):
    """Rows whose launch is waiting on a startup dialog that the user has now answered."""
    return [row for row in rows if row["record"].get("stage") == "startup_blocked" and row["status"] in ("idle", "done") and row["record"].get("task")]


STATUS_ORDER = {"blocked": 0, "done": 1, "working": 2, "unknown": 3, "idle": 4}
STATUS_LABEL = {"blocked": "Input", "done": "Ready", "working": "Working", "unknown": "Unknown", "idle": "Idle"}
STATUS_GLYPH = {"blocked": "●", "done": "✓", "working": "◐", "unknown": "?", "idle": "○"}
GROUP_LABEL = {"blocked": "Needs input", "done": "Ready", "working": "Working", "unknown": "Unknown", "idle": "Idle"}


def _branch_lookup(inventory):
    lookup = {}
    for project in (inventory or {}).get("projects", []):
        for checkout in project.get("checkouts", []):
            lookup[checkout["path"]] = project["name"], checkout.get("branch", ""), checkout.get("linked", False)
    return lookup


def inbox_rows(machine, agents, records, inventory=None):
    known = {r.get("pane_id"): r for r in records if r.get("machine_id") == machine.id and r.get("session", "default") == machine.session}
    branches = _branch_lookup(inventory)
    rows = []
    for agent in agents:
        record = known.get(agent["pane_id"], {})
        cwd = agent.get("foreground_cwd") or agent.get("cwd") or ""
        project, branch, linked = branches.get(cwd, (None, "", False))
        if record.get("branch"):
            branch, linked = record["branch"], record.get("workspace") == "worktree" or linked
        live_title = agent.get("terminal_title_stripped") or ""
        if re.match(r"^[\w.-]+@[\w.-]+:", live_title):
            live_title = ""  # a shell prompt title says nothing about the task
        title = record.get("title") or agent.get("tokens", {}).get("thread") or live_title or agent.get("name") or agent["pane_id"]
        status = agent.get("agent_status", "unknown")
        unverified = bool(record.get("unverified")) and status not in ("working", "blocked", "done")
        note = "answer the startup prompt, the task follows" if record.get("stage") == "startup_blocked" else "sending the task" if record.get("stage") == "submitting" and record.get("task") else "unverified send" if unverified else ""
        rows.append({"machine": machine, "pane_id": agent["pane_id"], "title": title, "live_title": live_title if live_title != title else "", "project": record.get("project") or project or Path(cwd).name or "Unknown project", "branch": branch, "worktree": linked, "harness": agent.get("agent") or "agent", "status": status, "sequence": agent.get("state_change_seq", 0), "record": record, "unverified": unverified, "note": note})
    occupied = {row["pane_id"] for row in rows}
    for pane_id, record in known.items():
        if not pane_id or pane_id in occupied:
            continue
        if record.get("stage") == "needs_attention":
            note = "setup failed: " + (record.get("error") or "").splitlines()[0]
        elif record.get("stage") in ("startup_blocked", "submitting") and record.get("task"):
            note = "the agent exited before the task was sent"
        else:
            continue
        rows.append({"machine": machine, "pane_id": pane_id, "title": record["title"], "live_title": "", "project": record["project"], "branch": record.get("branch", ""), "worktree": record.get("workspace") == "worktree", "harness": record["harness"], "status": "blocked", "sequence": 0, "record": record, "setup": True, "unverified": False, "note": note})
    return rows


def sort_rows(rows):
    # State sequence counters are scoped to one server. Don't compare them across hosts.
    return sorted(rows, key=lambda row: (STATUS_ORDER.get(row["status"], 3), row["project"].lower(), row["machine"].label, -row["sequence"]))
