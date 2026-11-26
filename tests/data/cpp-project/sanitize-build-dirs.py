#!/usr/bin/env python3
"""Strip machine-local paths from fixture build directories.

The fixture commits build directories as test input.  Raw CMake/Ninja output
contains absolute paths, Nix store hashes, PIDs, and other host-specific data.
This script keeps the files the analyzer needs and rewrites the rest so the
directories are small, portable, and safe to commit.

Usage:
    ./sanitize-build-dirs.py [build-dir ...]

Without arguments, all build-clang-* directories next to this script are
sanitized in place.  Run it after build-traces.sh.
"""

from __future__ import annotations

import json
import os
import re
import shlex
import subprocess
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
NIX_HASH = re.compile(r"/nix/store/[0-9a-z]{32}-")
HOME_PATH = re.compile(r"(/home/|/Users/)[^/\s\"']+")
ZERO_HASH = "0" * 32
SYSTEM_PREFIX = f"/nix/store/{ZERO_HASH}-system"


def repo_root() -> Path:
    out = subprocess.check_output(
        ["git", "-C", str(SCRIPT_DIR), "rev-parse", "--show-toplevel"],
        text=True,
    )
    return Path(out.strip())


def write_text(path: Path, text: str) -> None:
    # CMake writes some of its generated files read-only.
    path.chmod(path.stat().st_mode | 0o200)
    path.write_text(text)


def is_relative_to(path: Path, root: Path) -> bool:
    try:
        path.relative_to(root)
        return True
    except ValueError:
        return False


def sanitize_external_path(path: str) -> str:
    path = NIX_HASH.sub(f"/nix/store/{ZERO_HASH}-", path)
    path = path.replace("/run/current-system", SYSTEM_PREFIX)
    path = HOME_PATH.sub(lambda m: m.group(1) + "user", path)
    return path


def sanitize_trace_string(value: str, root: Path) -> str:
    prefix = str(root) + "/"
    if value.startswith(prefix):
        value = value[len(prefix) :]
    elif value == str(root):
        value = "."
    return sanitize_external_path(value)


def sanitize_json_value(value, root: Path):
    if isinstance(value, str):
        return sanitize_trace_string(value, root)
    if isinstance(value, list):
        return [sanitize_json_value(v, root) for v in value]
    if isinstance(value, dict):
        return {k: sanitize_json_value(v, root) for k, v in value.items()}
    return value


def sanitize_trace(path: Path, root: Path) -> None:
    data = json.loads(path.read_text())
    data = sanitize_json_value(data, root)

    events = data.get("traceEvents", [])
    lane_ids: dict[tuple[object, object], int] = {}
    for event in events:
        key = (event.get("pid"), event.get("tid"))
        lane = lane_ids.setdefault(key, len(lane_ids) + 1)
        event["pid"] = lane
        event["tid"] = lane
    if "beginningOfTime" in data:
        data["beginningOfTime"] = 0

    write_text(path, json.dumps(data, separators=(",", ":"), ensure_ascii=False) + "\n")


def rewrite_arg(arg: str, build_dir: Path, root: Path) -> str:
    if os.path.isabs(arg):
        path = Path(arg)
        if is_relative_to(path, root):
            return os.path.relpath(path, build_dir)
        name = path.name
        if name in {"clang", "clang++", "cc", "c++"}:
            return name
        return sanitize_external_path(arg)

    # Keep option prefixes attached to rewritten paths (-I..., -isystem...).
    for prefix in ("-I", "-isystem", "-iquote", "-idirafter", "-include", "-imacros"):
        if arg.startswith(prefix) and len(arg) > len(prefix):
            value = rewrite_arg(arg[len(prefix) :], build_dir, root)
            return prefix + value
    return arg


def sanitize_compile_commands(path: Path, build_dir: Path, root: Path) -> None:
    commands = json.loads(path.read_text())
    for entry in commands:
        directory = Path(entry.get("directory", ""))
        file = Path(entry.get("file", ""))

        if is_relative_to(directory, root):
            entry["directory"] = os.path.relpath(directory, root)
        else:
            entry["directory"] = sanitize_external_path(str(directory))

        if is_relative_to(file, root):
            entry["file"] = os.path.relpath(file, root)
        else:
            entry["file"] = sanitize_external_path(str(file))

        if isinstance(entry.get("output"), str):
            output = Path(entry["output"])
            if is_relative_to(output, build_dir):
                entry["output"] = os.path.relpath(output, build_dir)
            elif is_relative_to(output, root):
                entry["output"] = os.path.relpath(output, build_dir)
            else:
                entry["output"] = sanitize_external_path(entry["output"])

        if isinstance(entry.get("arguments"), list):
            entry["arguments"] = [
                rewrite_arg(str(arg), build_dir, root) for arg in entry["arguments"]
            ]
        if isinstance(entry.get("command"), str):
            args = shlex.split(entry["command"])
            entry["command"] = shlex.join(
                rewrite_arg(str(arg), build_dir, root) for arg in args
            )

    write_text(path, json.dumps(commands, indent=2, ensure_ascii=False) + "\n")


def sanitize_build_ninja(path: Path, build_dir: Path, root: Path) -> None:
    text = path.read_text()
    text = text.replace(str(build_dir), ".")
    text = text.replace(str(root), os.path.relpath(root, build_dir))
    text = sanitize_external_path(text)
    write_text(path, text)


def sanitize_cmake_cache(path: Path) -> None:
    keys = {
        "CMAKE_CACHE_MAJOR_VERSION",
        "CMAKE_CACHE_MINOR_VERSION",
        "CMAKE_CACHE_PATCH_VERSION",
        "CMAKE_GENERATOR",
    }
    kept: list[str] = []
    for line in path.read_text().splitlines():
        match = re.match(r"^([A-Za-z0-9_]+)(:[A-Z]+)?=", line)
        if match and match.group(1) in keys:
            kept.append(line)
    write_text(path, "\n".join(kept) + "\n")


def sanitize_cmake_compiler_file(path: Path) -> None:
    keys = {
        "CMAKE_CXX_COMPILER_ID",
        "CMAKE_CXX_COMPILER_VERSION",
        "CMAKE_C_COMPILER_ID",
        "CMAKE_C_COMPILER_VERSION",
        "CMAKE_CXX_COMPILER_LINKER_ID",
        "CMAKE_CXX_COMPILER_LINKER_VERSION",
    }
    kept: list[str] = []
    for line in path.read_text().splitlines():
        match = re.match(r"^set\(([A-Za-z0-9_]+) (.*)\)$", line.strip())
        if match and match.group(1) in keys:
            kept.append(line.strip())
    write_text(path, "\n".join(kept) + "\n")


def is_kept(rel: Path) -> bool:
    if rel.as_posix() in {"build.ninja", "compile_commands.json", "CMakeCache.txt"}:
        return True
    if str(rel).endswith(".cpp.json"):
        return True
    if re.fullmatch(r"CMakeFiles/[^/]+/CMakeCXXCompiler\.cmake", rel.as_posix()):
        return True
    if rel.as_posix().startswith(".cmake/api/v1/reply/"):
        return True
    return False


def remove_empty_dirs(build_dir: Path) -> None:
    for current, _, _ in os.walk(build_dir, topdown=False):
        path = Path(current)
        if path == build_dir:
            continue
        try:
            path.rmdir()
        except OSError:
            pass


def sanitize_build_dir(build_dir: Path, root: Path) -> None:
    build_dir = build_dir.resolve()

    # Keep only analyzer inputs: traces, compile commands, build info and the
    # Ninja target graph.
    for path in sorted(build_dir.rglob("*"), reverse=True):
        if path.is_file() and not is_kept(path.relative_to(build_dir)):
            path.unlink()

    for path in build_dir.rglob("*.cpp.json"):
        sanitize_trace(path, root)

    compile_commands = build_dir / "compile_commands.json"
    if compile_commands.is_file():
        sanitize_compile_commands(compile_commands, build_dir, root)

    build_ninja = build_dir / "build.ninja"
    if build_ninja.is_file():
        sanitize_build_ninja(build_ninja, build_dir, root)

    cmake_cache = build_dir / "CMakeCache.txt"
    if cmake_cache.is_file():
        sanitize_cmake_cache(cmake_cache)

    for compiler_file in build_dir.glob("CMakeFiles/*/CMakeCXXCompiler.cmake"):
        sanitize_cmake_compiler_file(compiler_file)

    remove_empty_dirs(build_dir)


def main(argv: list[str]) -> int:
    if any(arg in {"-h", "--help"} for arg in argv):
        print(__doc__.strip())
        return 0

    root = repo_root()
    if argv:
        build_dirs = [Path(arg) for arg in argv]
    else:
        build_dirs = sorted(SCRIPT_DIR.glob("build-clang-*"))

    for build_dir in build_dirs:
        if not build_dir.is_dir():
            print(f"warning: not a directory: {build_dir}", file=sys.stderr)
            continue
        print(f"sanitizing {build_dir}")
        sanitize_build_dir(build_dir, root)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
