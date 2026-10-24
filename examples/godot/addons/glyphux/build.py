#!/usr/bin/env python3
"""Builds the Glyphux GDExtension library into this addon's bin/ folder.

Run it from anywhere in a Glyphux checkout; it needs Rust's cargo (https://rustup.rs):

    python examples/godot/addons/glyphux/build.py          # optimized, for the editor and exports
    python examples/godot/addons/glyphux/build.py --dev    # unoptimized, faster to compile (editor only)
    python examples/godot/addons/glyphux/build.py --project ../my_game
                                                           # also install the addon into a project

This folder is the addon, inside the demo project that uses it; rust/ holds the library's
crate (a member of the Glyphux workspace). The build also writes the generated shaders
(shaders/generated). Installing into a project copies what Godot needs, without rust/ and
this script. By default the editor and debug exports load the optimized library too: glyph
preparation is CPU work that an unoptimized build makes noticeably slow.
"""

import argparse
import json
import platform
import shutil
import subprocess
import sys
from pathlib import Path

ADDON = Path(__file__).resolve().parent
CRATE = ADDON / "rust"
# What a project's addons/glyphux needs at run time.
RUNTIME = ["glyphux.gdextension", "bin", "icons", "shaders", "README.md", "LICENSE"]


def godot_platform():
    """Godot's name for this OS and CPU, and the library file names cargo and Godot use."""
    system = platform.system()
    machine = platform.machine().lower()
    arch = {"amd64": "x86_64", "x86_64": "x86_64", "arm64": "arm64", "aarch64": "arm64"}.get(machine)
    if arch is None:
        sys.exit(f"build.py: unsupported CPU '{machine}'")
    if system == "Windows":
        return "glyphux_godot.dll", lambda slot: f"glyphux.windows.{slot}.{arch}.dll"
    if system == "Linux":
        return "libglyphux_godot.so", lambda slot: f"libglyphux.linux.{slot}.{arch}.so"
    if system == "Darwin":
        return "libglyphux_godot.dylib", lambda slot: f"libglyphux.macos.{slot}.dylib"
    sys.exit(f"build.py: unsupported OS '{system}'")


def cargo(args, capture=False):
    command = ["cargo", *args]
    if not capture:
        print("+", " ".join(command), flush=True)
    try:
        result = subprocess.run(command, cwd=CRATE, check=True, capture_output=capture, text=capture)
    except FileNotFoundError:
        sys.exit("build.py: cargo not found; install Rust from https://rustup.rs")
    except subprocess.CalledProcessError as e:
        sys.exit(e.returncode)
    return result.stdout


def target_directory():
    metadata = json.loads(cargo(["metadata", "--format-version", "1", "--no-deps"], capture=True))
    return Path(metadata["target_directory"])


def copy(source, target):
    target.parent.mkdir(parents=True, exist_ok=True)
    try:
        shutil.copy2(source, target)
    except PermissionError:
        sys.exit(f"build.py: {target} is in use; close the running game (or the editor) and build again")
    print(f"  {target}")


def install(project):
    """Copies the addon's run time files into project/addons/glyphux."""
    project = project.resolve()
    if not (project / "project.godot").is_file():
        sys.exit(f"build.py: {project} is not a Godot project (no project.godot)")
    target = project / "addons" / "glyphux"
    if target == ADDON:
        return
    print(f"installing into {target}")
    for name in RUNTIME:
        source = ADDON / name
        files = sorted(source.rglob("*")) if source.is_dir() else [source]
        for f in files:
            if f.is_file():
                copy(f, target / f.relative_to(ADDON))


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--dev", action="store_true", help="unoptimized build, for the editor and debug exports only")
    parser.add_argument("--project", type=Path, help="also install the addon into this Godot project")
    parser.add_argument("--check", action="store_true", help="check formatting and lints instead of building (CI)")
    args = parser.parse_args()

    if args.check:
        cargo(["fmt", "-p", "glyphux-godot", "--check"])
        cargo(["clippy", "-p", "glyphux-godot", "--all-targets", "--", "-D", "warnings"])
        return

    library, file_name = godot_platform()
    # The workspace's `godot` profile: release, with unwinding (see the root Cargo.toml).
    profile = "debug" if args.dev else "godot"
    cargo(["build", "-p", "glyphux-godot"] + ([] if args.dev else ["--profile", "godot"]))
    built = target_directory() / profile / library
    slots = ["debug"] if args.dev else ["debug", "release"]
    print("copying the library")
    for slot in slots:
        copy(built, ADDON / "bin" / file_name(slot))
    if args.project:
        install(args.project)


if __name__ == "__main__":
    main()
