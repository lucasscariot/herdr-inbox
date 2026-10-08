#!/usr/bin/env python3
"""Rebuild the optional inbox client from the pinned Herdr source and patch."""

import argparse
import os
import shutil
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
COMMIT = "7b116c05bfda646af39d2524c54e70c751f57ee8"


def run(args, cwd=None, env=None):
    subprocess.run(args, cwd=cwd, env=env, check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--zig", help="Path to Zig 0.16.0")
    args = parser.parse_args()
    checkout = ROOT / "build/herdr"
    checkout.parent.mkdir(parents=True, exist_ok=True)
    if not checkout.exists():
        run(["git", "clone", "--depth", "1", "--branch", "v0.9.3", "https://github.com/herdrdev/herdr.git", str(checkout)])
    commit = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=checkout, text=True).strip()
    if commit != COMMIT:
        raise SystemExit("Build checkout does not match the pinned Herdr 0.9.3 commit")
    patch = ROOT / "patches/herdr-0.9.3-inbox.patch"
    applied = subprocess.run(["git", "apply", "--reverse", "--check", str(patch)], cwd=checkout, capture_output=True)
    if applied.returncode:
        run(["git", "apply", "--check", str(patch)], cwd=checkout)
        run(["git", "apply", str(patch)], cwd=checkout)
    env = dict(os.environ)
    env["PATH"] = str(Path.home() / ".cargo/bin") + os.pathsep + env.get("PATH", "")
    if args.zig:
        env["ZIG"] = str(Path(args.zig).resolve())
    run(["just", "build"], cwd=checkout, env=env)
    destination = ROOT / "build/herdr-inbox-client"
    shutil.copy2(checkout / "target/release/herdr", destination)
    print(destination)


if __name__ == "__main__":
    main()
