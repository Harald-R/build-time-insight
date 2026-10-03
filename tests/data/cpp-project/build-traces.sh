#!/usr/bin/env bash
# Configure and build the fixture with clang's -ftime-trace so every
# translation unit produces a <tu>.json trace file in the build directory.
#
# Usage:
#   ./build-traces.sh [build-dir]                 # one build with the system clang++
#   ./build-traces.sh --build-all [prefix]        # one build per clang version; creates (or
#                                                 # repairs) the conda env bti-clang<NN>, then
#                                                 # builds inside the activated env, into
#                                                 # <prefix><NN> (default prefix: build-clang-)
#
# Afterwards each build directory contains:
#   CMakeFiles/.../*.cpp.json            # one trace file per translation unit
#   compile_commands.json                # needed by `--header-usage`
#
# The analyzer is then run against a build directory, e.g.:
#   build-time-insight analyze build-clang-18 --header-usage
set -euo pipefail
cd "$(dirname "$0")"

# clang versions to build with under --build-all (matching conda envs bti-clang<NN>).
CLANG_VERSIONS=${BTI_CLANG_VERSIONS:-"21.1.8 22.1.8"}
BUILD_ALL=0
PREFIX="build-clang-"
BUILD_DIR="build-traces"

while [ $# -gt 0 ]; do
    case "$1" in
        --build-all) BUILD_ALL=1 ;;
        -h|--help) sed -n '2,17p' "$0"; exit 0 ;;
        *) if [ "$BUILD_ALL" -eq 1 ]; then PREFIX="$1"; else BUILD_DIR="$1"; fi ;;
    esac
    shift
done

# Build with the clang++ that is currently first on PATH (inside --build-all
# this is the activated env's clang++; otherwise the system clang++). The
# absolute path also forces CMake to re-detect a different compiler than one
# cached in an existing build directory.
configure_and_build() {
    local dir="$1" compiler
    compiler="$(command -v clang++ || true)"
    if [ -z "$compiler" ] || [ ! -x "$compiler" ]; then
        echo "error: clang++ not found on PATH (is the conda env activated?)" >&2
        exit 1
    fi
    echo "  compiler: $compiler ($("$compiler" --version | head -1))"
    cmake -S . -B "$dir" -G Ninja -DCMAKE_CXX_COMPILER="$compiler" -DENABLE_FTIME_TRACE=ON
    cmake --build "$dir"
    echo
    echo "Trace files produced ($dir):"
    find "$dir" -name '*.cpp.json' | sort
}

# Create the conda env for a clang version if it does not exist.
ensure_conda_env() {
    local version="$1" env_name="bti-clang${1}"
    local base
    base="$(conda info --base)"
    if conda env list | awk '{print $1}' | grep -qx "$env_name"; then
        return 0
    fi
    echo "==> Creating conda env '$env_name' (clang $version)"
    conda create -y -n "$env_name" -c conda-forge \
        "clang=${version}.*" "clangxx=${version}.*" cmake ninja
}

build_all() {
    local version dir env_name base
    if ! command -v conda >/dev/null 2>&1; then
        echo "error: --build-all requires conda" >&2
        exit 1
    fi
    base="$(conda info --base)"
    for version in $CLANG_VERSIONS; do
        dir="${PREFIX}${version}"
        env_name="bti-clang${version}"
        ensure_conda_env "$version"
        echo "==> Building with clang $version -> $dir (env $env_name)"
        (
            source "$base/etc/profile.d/conda.sh"
            conda activate "$env_name"
            configure_and_build "$dir"
        )
    done
    echo
    echo "All clang builds complete. Analyze each directory, e.g.:"
    for version in $CLANG_VERSIONS; do
        echo "  build-time-insight analyze ${PREFIX}${version} --header-usage"
    done
}

if [ "$BUILD_ALL" -eq 1 ]; then
    build_all
else
    configure_and_build "$BUILD_DIR"
fi