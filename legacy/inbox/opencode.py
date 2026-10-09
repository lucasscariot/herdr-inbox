"""OpenCode 2 admits tasks through its native, session-scoped API."""

import json
import os

from .herdr import host_command, run_json


def api(machine, directory, operation, body=None, session=None, extra_args=()):
    argv = ["opencode", "api", operation]
    # A configured server must be the same server the native terminal attaches to.
    for index, argument in enumerate(extra_args):
        if argument == "--server" and index + 1 < len(extra_args):
            argv += ["--server", extra_args[index + 1]]
        elif argument.startswith("--server="):
            argv.append(argument)
    if session:
        argv += ["--param", "sessionID=" + session]
    if body is not None:
        argv += ["--data", json.dumps(body)]
    env = {key: value for key, value in os.environ.items() if not key.startswith("HERDR_")}
    return run_json(host_command(machine, argv, directory), env=env, cwd=directory if machine.id == "local" else None, timeout=30, allow_empty=operation in ("session.remove", "session.switchModel"))


def model_ref(model):
    provider, identifier = model.split("/", 1)
    identifier, separator, variant = identifier.partition("#")
    ref = {"providerID": provider, "id": identifier}
    if separator:
        ref["variant"] = variant
    return ref


def create(machine, record, model, extra_args):
    body = {"id": record["native_session_id"], "title": record["title"], "location": {"directory": record["cwd"]}}
    if model:
        body["model"] = model_ref(model)
    response = api(machine, record["cwd"], "session.create", body, extra_args=extra_args)
    if response.get("data", {}).get("id") != record["native_session_id"]:
        raise ValueError("OpenCode did not confirm the new session ID")


def select_model(machine, record, model, extra_args):
    api(machine, record["cwd"], "session.switchModel", {"model": model_ref(model)}, session=record["native_session_id"], extra_args=extra_args)


def prompt(machine, record, task, extra_args):
    response = api(machine, record["cwd"], "session.prompt", {"id": "msg_" + record["id"], "text": task}, session=record["native_session_id"], extra_args=extra_args)
    if response.get("data", {}).get("id") != "msg_" + record["id"]:
        raise ValueError("OpenCode did not confirm task acceptance")
