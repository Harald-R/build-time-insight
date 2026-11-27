#!/usr/bin/env python3
"""Strip machine-local paths from fixture build directories.

The fixture projects commit build directories as test input. Raw CMake/Ninja
output contains absolute paths, Nix store hashes, PIDs, and other host-specific
data. This script keeps the files the analyzer needs and rewrites the rest so
the directories are small, portable, safe to commit, and still analyzable.

Usage:
    ./sanitize-build-dirs.py [path ...]

Each path is searched non-recursively for build-clang-* directories. A path
that is itself a build-clang-* directory is sanitized directly. Without
arguments, the current working directory is searched. Run it after a fixture's
build-traces.sh.

Examples:
    ./sanitize-build-dirs.py tests/data/cpp-general-project
    ./sanitize-build-dirs.py tests/data/cpp-general-project tests/data/cpp-modules-project
    ./sanitize-build-dirs.py tests/data/cpp-general-project/build-clang-18

It keeps:
  - all Clang trace JSON, including *.cpp.json, *.cppm.json, and extra PCM
    phase traces emitted by clang <= 21
  - compile_commands.json
  - build.ninja for target timings
  - a minimal CMakeCache.txt and CMakeCXXCompiler.cmake for build info
  - CMake File API replies, when present

It removes object files, archives, binaries, .ninja_log, .ninja_deps, CMake
compiler-detection output, configuration logs, module .pcm files, and .modmap
response files.

It rewrites:
  - project paths to be relative to the repository or build directory
  - Nix store hashes to 000...000 and /run/current-system to a neutral prefix
  - home-directory names to "user"
  - trace PIDs/thread IDs to small stable IDs and beginningOfTime to 0

Trace analysis and target mapping work after sanitization. For the C++20
modules fixture, removing .pcm/.modmap means --header-usage replay of module
units requires rebuilding their BMIs first.
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


def is_trace_json(path: Path) -> bool:
    try:
        return b'"traceEvents"' in path.read_bytes()[:512]
    except OSError:
        return False


def is_kept(path: Path, rel: Path) -> bool:
    if rel.as_posix() in {"build.ninja", "compile_commands.json", "CMakeCache.txt"}:
        return True
    if path.suffix == ".json" and is_trace_json(path):
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
        if path.is_file() and not is_kept(path, path.relative_to(build_dir)):
            path.unlink()

    trace_paths = [path for path in build_dir.rglob("*.json") if is_trace_json(path)]
    for path in trace_paths:
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
    paths = [Path(arg) for arg in argv] if argv else [Path.cwd()]

    build_dirs: list[Path] = []
    for path in paths:
        if not path.is_dir():
            print(f"warning: not a directory: {path}", file=sys.stderr)
            continue
        if path.name.startswith("build-clang-"):
            build_dirs.append(path)
            continue
        found = sorted(path.glob("build-clang-*"))
        if not found:
            print(f"warning: no build-clang-* directories in {path}", file=sys.stderr)
        build_dirs.extend(found)

    # Deduplicate when both a parent and one of its build dirs were given.
    for build_dir in sorted({path.resolve() for path in build_dirs}):
        print(f"sanitizing {build_dir}")
        sanitize_build_dir(build_dir, root)
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
