#!/usr/bin/env python3
"""Herdr Inbox's plugin entrypoint and scriptable launcher."""

import argparse
import curses
import json
import os
import sys
from concurrent.futures import ThreadPoolExecutor

from inbox.herdr import Herdr, HerdrError
from inbox.inventory import discover
from inbox.store import Store
from inbox.threads import inbox_rows, launch, resumable, resume, sort_rows
from inbox.ui import UI


def main():
    parser = argparse.ArgumentParser(description="Launch and switch agent threads across Herdr machines.")
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("open", "ui"):
        command = commands.add_parser(name)
        command.add_argument("--view", choices=["new", "inbox"], default="new")
        if name == "ui":
            command.add_argument("--demo", action="store_true", help="Preview the UI without contacting Herdr")
    commands.add_parser("discover", help="Print projects, worktrees, and installed harnesses for every saved machine")
    commands.add_parser("list", help="Print the live agent inbox as JSON")
    commands.add_parser("resume", help="Send the task of every launch that stopped at a startup dialog whose agent is now idle")
    command = commands.add_parser("launch", help="Create a thread without opening the composer")
    command.add_argument("--machine", required=True, help="Local, saved machine ID, or exact label")
    command.add_argument("--project", required=True, help="Project name or absolute path on the selected machine")
    command.add_argument("--harness", required=True)
    command.add_argument("--model", default="", help="Native model ID; omit to use the harness default")
    command.add_argument("--thinking", default="", help="Native thinking or effort level; omit to use the harness default")
    where = command.add_mutually_exclusive_group()
    where.add_argument("--worktree", nargs="?", const="", metavar="BRANCH", help="Run in a new Git worktree; the branch defaults to a name derived from the task")
    where.add_argument("--checkout", metavar="PATH", help="Run in this existing checkout or worktree of the project")
    command.add_argument("--base", default="", help="Base ref for a new worktree; defaults to the current HEAD")
    command.add_argument("--task", required=True)
    args = parser.parse_args()
    if os.environ.get("HERDR_ENV") != "1" and not getattr(args, "demo", False):
        parser.error("Run this inside Herdr. For a UI preview use: python3 main.py ui --demo")
    herdr, store = Herdr(), Store()
    if args.command == "open":
        print(json.dumps(herdr.open(args.view)))
    elif args.command == "ui":
        curses.wrapper(lambda screen: UI(screen, herdr, store, args.view, args.demo).run())
        if not args.demo:
            # The terminal is restored and any launch has completed. Executor
            # workers doing discovery must not keep the composer alive at exit.
            os._exit(0)
    elif args.command == "discover":
        machines = herdr.machines()
        result = {}
        with ThreadPoolExecutor(max_workers=4) as pool:
            futures = [(machine, pool.submit(discover, machine, store)) for machine in machines]
            for machine, future in futures:
                try:
                    result[machine.label] = future.result()
                except Exception as error:
                    result[machine.label] = {"error": str(error)}
        print(json.dumps(result, indent=2, ensure_ascii=False))
    elif args.command in ("list", "resume"):
        rows, errors = [], {}
        records = store.threads()
        with ThreadPoolExecutor(max_workers=4) as pool:
            futures = [(machine, pool.submit(herdr.agents, machine)) for machine in herdr.machines()]
            for machine, future in futures:
                try:
                    rows.extend(inbox_rows(machine, future.result(), records, store.cached_inventory(machine)))
                except HerdrError as error:
                    errors[machine.label] = str(error)
        rows = sort_rows(rows)
        if args.command == "resume":
            resumed = [resume(herdr, store, row["record"]) for row in resumable(rows)]
            print(json.dumps({"resumed": resumed, "errors": errors}, indent=2, ensure_ascii=False))
            return
        for row in rows:
            row["machine"] = row["machine"].__dict__
        print(json.dumps({"threads": rows, "errors": errors}, indent=2, ensure_ascii=False))
    elif args.command == "launch":
        machine = next((m for m in herdr.machines() if args.machine in (m.id, m.label)), None)
        if machine is None:
            parser.error("Unknown or disabled machine: " + args.machine)
        inventory = discover(machine, store)
        project = next((p for p in inventory["projects"] if args.project in (p["name"], p["path"])), None)
        if not project:
            parser.error("Project not found on " + machine.label)
        if args.harness not in inventory["harnesses"]:
            parser.error("Harness not installed on " + machine.label)
        if args.worktree is not None:
            workspace = {"mode": "worktree", "branch": args.worktree, "base": args.base}
        else:
            workspace = {"mode": "checkout", "path": args.checkout or project["path"]}
        record = launch(herdr, store, machine, project, args.harness, args.task, model=args.model, inventory=inventory, thinking=args.thinking, workspace=workspace)
        print(json.dumps(record, indent=2, ensure_ascii=False))


if __name__ == "__main__":
    try:
        main()
    except (HerdrError, ValueError, OSError) as error:
        print(str(error), file=sys.stderr)
        sys.exit(1)
