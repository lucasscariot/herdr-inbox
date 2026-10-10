# Discovery probe for Herdr Inbox. Runs on each machine with Python 3.9+,
# from source, so nothing needs installing there. Reads one JSON argument:
#   {"roots": [...], "depth": 0-3, "projects": [{"path", "name"}],
#    "executables": {harness: executable}, "include_models": bool}
# and prints {"projects": [...], "harnesses": [...], "models": {...}}.
import json, os, pathlib, shutil, sys, subprocess, re
from concurrent.futures import ThreadPoolExecutor
try:
    import tomllib
except ImportError:  # Python 3.9/3.10 on remote machines.
    tomllib = None
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

executables = dict(settings["executables"])
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
    thinking, thinking_by_model = [], {}
    if thinking_flag and thinking_flag in help_text:
        description = help_text.split(thinking_flag, 1)[1].split("\n  --", 1)[0]
        thinking = [level for level in ("off", "minimal", "low", "medium", "high", "xhigh", "max") if re.search(r"\b" + level + r"\b", description)]
    else:
        thinking_flag = ""
    if kind == "codex":
        home = pathlib.Path(os.environ.get("CODEX_HOME", str(pathlib.Path.home() / ".codex"))).expanduser()
        try:
            cache = json.loads((home / "models_cache.json").read_text())
            for model in cache.get("models", []) if isinstance(cache, dict) else []:
                if not isinstance(model, dict) or model.get("visibility") != "list" or not isinstance(model.get("slug"), str) or not model["slug"]:
                    continue
                identifier = model["slug"]
                label = model.get("display_name")
                choices.append({"id": identifier, "label": label if isinstance(label, str) else identifier})
                levels = model.get("supported_reasoning_levels")
                if isinstance(levels, list):
                    thinking_by_model[identifier] = list(dict.fromkeys(
                        entry["effort"] for entry in levels
                        if isinstance(entry, dict) and isinstance(entry.get("effort"), str) and entry["effort"]
                    ))
            thinking = list(dict.fromkeys(level for levels in thinking_by_model.values() for level in levels))
            if thinking and re.search(r"--config\b", help_text):
                thinking_flag = "--config"
            else:
                thinking, thinking_by_model = [], {}
        except (OSError, ValueError, TypeError):
            pass
        try:
            config_text = (home / "config.toml").read_text()
            if tomllib:
                model = tomllib.loads(config_text).get("model")
                if isinstance(model, str):
                    default = model
            else:
                # Never match a model inside a profile or provider table.
                root_config = re.split(r"(?m)^\s*\[", config_text, 1)[0]
                model = re.search(r'''(?m)^\s*model\s*=\s*["']([^"']+)["']''', root_config)
                if model:
                    default = model.group(1)
        except (OSError, ValueError):
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
    return kind, {"choices": choices, "selectable": selectable, "session_api": api, "default": default, "thinking_flag": thinking_flag, "thinking": thinking, "thinking_by_model": thinking_by_model}

models = {}
if settings.get("include_models", True):
    with ThreadPoolExecutor(max_workers=4) as pool:
        models = dict(pool.map(catalog, harnesses))
for project in projects.values():
    checkouts = sorted(project["checkouts"].values(), key=lambda c: (c["linked"], c["branch"].lower(), c["path"]))
    project["checkouts"] = checkouts
    project["branch"] = next((c["branch"] for c in checkouts if not c["linked"]), "")
print(json.dumps({"projects": sorted(projects.values(), key=lambda p: (p["name"].lower(), p["path"])), "harnesses": harnesses, "models": models}))
