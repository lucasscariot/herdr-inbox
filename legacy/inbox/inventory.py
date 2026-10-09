"""Bounded project discovery on Local and Herdr's saved SSH machines.

A project is one Git repository. Its linked worktrees, including the ones Herdr
creates under ``~/.herdr/worktrees``, are grouped under the repository instead
of appearing as separate projects.
"""

import json
import time

from .herdr import host_command, run_json

MODEL_CACHE_SECONDS = 15 * 60

# Herdr's supported agent kinds and the executable each one installs. A
# ``harness_executables`` mapping in config.json overrides or extends this.
HARNESS_EXECUTABLES = {
    "claude": "claude",
    "codex": "codex",
    "opencode": "opencode",
    "pi": "pi",
    "gemini": "gemini",
    "copilot": "copilot",
    "amp": "amp",
    "cursor": "agent",
    "grok": "grok",
    "kimi": "kimi",
    "kiro": "kiro-cli",
    "droid": "droid",
    "hermes": "hermes",
    "kilo": "kilo",
    "qwen": "qwen",
    "cline": "cline",
    "devin": "devin",
    "agy": "agy",
    "omp": "omp",
    "mastracode": "mastracode",
    "qodercli": "qodercli",
    "letta": "letta",
    "maki": "maki",
    "muse": "muse",
}

# Runs on the selected host with Python 3.9+, without installing this plugin there.
PROBE = r'''
import json, os, pathlib, shutil, sys, subprocess, re
from concurrent.futures import ThreadPoolExecutor
settings = json.loads(sys.argv[1])
markers = ("package.json", "Cargo.toml", "pyproject.toml", "mix.exs", "go.mod", "CMakeLists.txt", "pubspec.yaml", "Gemfile", "build.gradle", "Package.swift")
skipped = ("node_modules", "target", "vendor", "build", "dist", "Pods", "DerivedData", "venv")
projects = {}

def read_head(gitdir):
    try:
        ref = (gitdir / "HEAD").read_text().strip()
    except OSError:
        return ""
    return ref[16:] if ref.startswith("ref: refs/heads/") else ref[:10]

def git_info(directory):
    """Return (repo_root, common_git_dir, own_git_dir) or None when not a Git checkout."""
    git = directory / ".git"
    try:
        if git.is_dir():
            return directory, git, git
        if git.is_file():
            target = git.read_text().strip()
            if not target.startswith("gitdir:"):
                return None
            gitdir = pathlib.Path(target[7:].strip())
            if not gitdir.is_absolute():
                gitdir = (directory / gitdir).resolve()
            if gitdir.parent.name == "worktrees" and gitdir.parent.parent.name == ".git":
                common = gitdir.parent.parent
                return common.parent, common, gitdir
            return directory, gitdir, gitdir
    except OSError:
        return None
    return None

def project_for(root, name=None):
    key = str(root)
    project = projects.get(key)
    if not project:
        project = projects[key] = {"name": name or root.name, "path": key, "checkouts": {}}
    return project

def add_checkout(project, path, branch, linked):
    project["checkouts"][str(path)] = {"path": str(path), "branch": branch, "linked": linked}

def register(directory, name=None):
    info = git_info(directory)
    if not info:
        if any((directory / m).is_file() for m in markers):
            project = project_for(directory.resolve(), name)
            add_checkout(project, directory.resolve(), "", False)
            return True
        return False
    root, common, own = info
    root = root.resolve()
    project = project_for(root, name)
    add_checkout(project, root, read_head(common), False)
    if own != common:
        add_checkout(project, directory.resolve(), read_head(own), True)
    # Linked worktrees live anywhere, including ~/.herdr/worktrees. git keeps a pointer to each.
    try:
        for entry in (common / "worktrees").iterdir():
            try:
                checkout = pathlib.Path((entry / "gitdir").read_text().strip()).parent
            except OSError:
                continue
            if checkout.is_dir():
                add_checkout(project, checkout.resolve(), read_head(entry), True)
    except OSError:
        pass
    return True

def visit(directory, remaining):
    if directory.name.startswith(".") or directory.name in skipped:
        return
    try:
        if register(directory):
            return
        if remaining > 0:
            for child in sorted(directory.iterdir()):
                if child.is_dir() and (not child.is_symlink() or git_info(child) or any((child / m).is_file() for m in markers)):
                    visit(child, remaining - 1)
    except (PermissionError, FileNotFoundError, OSError):
        return

for root in settings["roots"]:
    directory = pathlib.Path(root).expanduser()
    if directory.is_dir():
        visit(directory, settings["depth"])
for project in settings.get("projects", []):
    directory = pathlib.Path(project["path"]).expanduser()
    if directory.is_dir():
        register(directory, project.get("name"))

executables = dict(__EXECUTABLES__)
executables.update(settings.get("executables", {}))
harnesses = [kind for kind, executable in executables.items() if shutil.which(executable)]

def output(argv, timeout=5):
    try:
        env = {key: value for key, value in os.environ.items() if not key.startswith("HERDR_")}
        result = subprocess.run(argv, stdin=subprocess.DEVNULL, start_new_session=True, env=env, capture_output=True, text=True, timeout=timeout)
        return (result.stdout or result.stderr) if result.returncode == 0 else ""
    except (OSError, subprocess.TimeoutExpired):
        return ""

def catalog(kind):
    choices, api, default = [], False, ""
    executable = executables[kind]
    help_text = output([executable, "--help"])
    selectable = bool(re.search(r"--model\b", help_text))
    thinking_flag = "--thinking" if kind == "pi" else "--effort" if kind == "claude" else ""
    thinking = []
    if thinking_flag and thinking_flag in help_text:
        description = help_text.split(thinking_flag, 1)[1].split("\n  --", 1)[0]
        thinking = [level for level in ("off", "minimal", "low", "medium", "high", "xhigh", "max") if re.search(r"\b" + level + r"\b", description)]
    else:
        thinking_flag = ""
    if kind == "codex":
        try:
            cache = json.loads((pathlib.Path.home() / ".codex/models_cache.json").read_text())
            choices = [{"id": m["slug"], "label": m.get("display_name", m["slug"])} for m in cache.get("models", []) if m.get("visibility") == "list"]
        except (OSError, ValueError, KeyError):
            pass
    elif kind == "claude":
        choices = [{"id": name, "label": name.capitalize()} for name in ("opus", "sonnet", "haiku", "fable")]
    elif kind == "opencode":
        choices = [{"id": line.strip(), "label": line.strip()} for line in output([executable, "models"], timeout=12).splitlines() if re.fullmatch(r"[^\s/]+/[^\s]+", line.strip())]
        version = re.search(r"\bv?(\d+)\.\d+", output([executable, "--version"]))
        api = bool(version and int(version.group(1)) >= 2)
        selectable = selectable or api
        try:
            state = pathlib.Path(os.environ.get("XDG_STATE_HOME", str(pathlib.Path.home() / ".local/state"))) / "opencode/model.json"
            saved = json.loads(state.read_text())
            recent = saved.get("recent", [])
            if recent:
                candidate = recent[0]["providerID"] + "/" + recent[0]["modelID"]
                if candidate in {choice["id"] for choice in choices}:
                    default = candidate
                    variant = saved.get("variant", {}).get(candidate, "default")
                    if variant != "default":
                        default += "#" + variant
        except (OSError, ValueError, KeyError):
            pass
    elif kind == "pi":
        for line in output([executable, "--offline", "--list-models"]).splitlines():
            cells = line.split()
            if len(cells) >= 4 and cells[0] != "provider":
                identifier = cells[0] + "/" + cells[1]
                choices.append({"id": identifier, "label": identifier})
    return kind, {"choices": choices, "selectable": selectable, "session_api": api, "default": default, "thinking_flag": thinking_flag, "thinking": thinking}

models = {}
if settings.get("include_models", True):
    with ThreadPoolExecutor(max_workers=4) as pool:
        models = dict(pool.map(catalog, harnesses))
for project in projects.values():
    checkouts = sorted(project["checkouts"].values(), key=lambda c: (c["linked"], c["branch"].lower(), c["path"]))
    project["checkouts"] = checkouts
    project["branch"] = next((c["branch"] for c in checkouts if not c["linked"]), "")
print(json.dumps({"projects": sorted(projects.values(), key=lambda p: (p["name"].lower(), p["path"])), "harnesses": harnesses, "models": models}))
'''.replace("__EXECUTABLES__", json.dumps(HARNESS_EXECUTABLES))


def models_fresh(inventory):
    return bool(inventory) and time.time() - inventory.get("models_at", 0) < MODEL_CACHE_SECONDS and bool(inventory.get("models"))


def discover(machine, store, include_models=True):
    override = store.machine_config(machine)
    projects = [p for p in store.config.get("projects", []) if p.get("machine", "local") in (machine.id, machine.label)]
    executables = dict(HARNESS_EXECUTABLES)
    for source in (store.config.get("harness_executables", {}), override.get("harness_executables", {})):
        if not isinstance(source, dict) or not all(isinstance(k, str) and isinstance(v, str) and v.strip() for k, v in source.items()):
            raise ValueError("harness_executables must map harness kinds to executable names")
        executables.update(source)
    settings = {"roots": override.get("roots", store.config.get("roots", ["~/Work", "~/Projects", "~/goinfre"])), "depth": override.get("depth", store.config.get("depth", 2)), "projects": projects, "include_models": include_models, "executables": executables}
    if not isinstance(settings["depth"], int) or not 0 <= settings["depth"] <= 3:
        raise ValueError("Project scan depth must be an integer between 0 and 3")
    args = ["python3", "-c", PROBE, json.dumps(settings)]
    result = run_json(host_command(machine, args), timeout=25)
    cached = store.cached_inventory(machine) or {}
    if include_models:
        result["models_at"] = time.time()
    else:
        result["models"] = cached.get("models", {})
        result["models_at"] = cached.get("models_at", 0)
    configured = override.get("models", store.config.get("models", {}))
    for harness, values in configured.items():
        if not isinstance(values, list) or not all(isinstance(value, str) and value.strip() for value in values):
            raise ValueError("models must contain lists of model IDs")
        catalog = result["models"].get(harness)
        if catalog:
            known = {choice["id"] for choice in catalog["choices"]}
            catalog["choices"].extend({"id": value, "label": value} for value in values if value not in known)
    result["updated_at"] = time.time()
    store.save_inventory(machine, result)
    return result


def checkout_for(project, path):
    """Return the checkout of ``project`` at ``path``, or its main checkout."""
    checkouts = project.get("checkouts") or [{"path": project["path"], "branch": project.get("branch", ""), "linked": False}]
    return next((c for c in checkouts if c["path"] == path), None) or next((c for c in checkouts if not c["linked"]), checkouts[0])
