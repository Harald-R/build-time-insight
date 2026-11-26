#!/usr/bin/env bash
# Configure and build the fixture with clang's -ftime-trace so every
# translation unit produces a <tu>.json trace file in the build directory.
#
# Afterwards each build directory contains:
#   CMakeFiles/.../*.cpp.json            # one trace file per translation unit
#   compile_commands.json                # needed by `--header-usage`
#
# The analyzer is then run against a build directory, e.g.:
#   build-time-insight analyze build-clang-18 --header-usage
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SCRIPT_PATH="$SCRIPT_DIR/$(basename "$0")"
cd "$SCRIPT_DIR"

usage() {
    cat <<'EOF'
Usage:
  ./build-traces.sh [build-dir]
      Build once with the clang++ currently on PATH.

  ./build-traces.sh --conda [--versions "<list>"] [prefix]
  ./build-traces.sh --nix [--versions "<list>"] [prefix]
      Build once per clang version using the selected backend.

Options:
  --versions <list>    Space-separated clang versions (default:
                       "21.1.8 22.1.8"), e.g. --versions "18 20 22".
  --conda              Use conda environments. Each version gets an env
                       bti-clang<version> with clang, CMake and Ninja.
  --nix                Use Nix. Each version is provided by nixpkgs'
                       llvmPackages_<major>.clang.
  --backend <name>     Select a backend: "conda" or "nix".
  -h, --help           Show this help.

Environment:
  BTI_NIXPKGS          Nix flake ref used by --nix (default: "nixpkgs").

Examples:
  ./build-traces.sh --conda
  ./build-traces.sh --nix
  ./build-traces.sh --nix --versions "18 20 22" myprefix-
  ./build-traces.sh --conda myprefix-   # custom directory prefix

A single Nix-provided version can also be built manually:

  nix shell nixpkgs#llvmPackages_18.clang nixpkgs#cmake nixpkgs#ninja \
      -c ./build-traces.sh build-clang-18
EOF
}

# clang versions to build with when a backend is selected.
CLANG_VERSIONS="21.1.8 22.1.8"
VERSIONS_SET=0
BACKEND=""
NIXPKGS="${BTI_NIXPKGS:-nixpkgs}"
PREFIX="build-clang-"
BUILD_DIR="build-traces"
INTERNAL_BUILD=""

while [ $# -gt 0 ]; do
    case "$1" in
        --build-all)
            echo "error: --build-all is no longer needed; select a backend with --conda or --nix" >&2
            exit 2
            ;;
        --conda) BACKEND=conda ;;
        --nix) BACKEND=nix ;;
        --versions)
            if [ $# -lt 2 ]; then
                echo "error: --versions requires a value, e.g. \"18 20 22\"" >&2
                exit 2
            fi
            CLANG_VERSIONS="$2"
            VERSIONS_SET=1
            shift
            ;;
        --backend)
            if [ $# -lt 2 ]; then
                echo "error: --backend requires a value (conda or nix)" >&2
                exit 2
            fi
            BACKEND="$2"
            shift
            ;;
        # Internal entry point used to run one build inside a `nix shell`.
        --internal-build)
            if [ $# -lt 2 ]; then
                echo "error: --internal-build requires a directory" >&2
                exit 2
            fi
            INTERNAL_BUILD="$2"
            shift
            ;;
        -h|--help) usage; exit 0 ;;
        *) if [ -n "$BACKEND" ]; then PREFIX="$1"; else BUILD_DIR="$1"; fi ;;
    esac
    shift
done

if [ -n "$BACKEND" ] && [ "$BACKEND" != conda ] && [ "$BACKEND" != nix ]; then
    echo "error: unknown backend '$BACKEND' (expected conda or nix)" >&2
    exit 2
fi

if [ "$VERSIONS_SET" -eq 1 ] && [ -z "$BACKEND" ]; then
    echo "error: --versions requires a backend: --conda or --nix" >&2
    exit 2
fi

if [ -n "$BACKEND" ] && [ -z "${CLANG_VERSIONS// /}" ]; then
    echo "error: --versions must not be empty" >&2
    exit 2
fi

# Build with the clang++ that is currently first on PATH (in a --conda or
# --nix build this is the activated env's or nix shell's clang++; otherwise the
# system clang++). The absolute path also forces CMake to re-detect a different
# compiler than one cached in an existing build directory.
configure_and_build() {
    local dir="$1" compiler
    compiler="$(command -v clang++ || true)"
    if [ -z "$compiler" ] || [ ! -x "$compiler" ]; then
        echo "error: clang++ not found on PATH (activate the conda env or use --nix)" >&2
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

build_versions_conda() {
    local version dir env_name base
    if ! command -v conda >/dev/null 2>&1; then
        echo "error: --conda requires conda (use --nix on NixOS)" >&2
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
}

# Build one version with the clang wrapper from nixpkgs. The version may be a
# full version (e.g. 21.1.8) or just a major (21); nixpkgs selects the exact
# patch release for that major.
build_versions_nix() {
    local version dir major
    if ! command -v nix >/dev/null 2>&1; then
        echo "error: --nix requires the nix command" >&2
        exit 1
    fi
    for version in $CLANG_VERSIONS; do
        major="${version%%.*}"
        if ! [[ "$major" =~ ^[0-9]+$ ]]; then
            echo "error: Nix backend needs a major clang version, got '$version'" >&2
            exit 1
        fi
        dir="${PREFIX}${version}"
        echo "==> Building with clang $version -> $dir (nix: $NIXPKGS#llvmPackages_${major}.clang)"
        nix shell \
            "${NIXPKGS}#llvmPackages_${major}.clang" \
            "${NIXPKGS}#cmake" \
            "${NIXPKGS}#ninja" \
            --command bash "$SCRIPT_PATH" --internal-build "$dir"
    done
}

print_analysis_commands() {
    local version
    echo
    echo "All clang builds complete. Analyze each directory, e.g.:"
    for version in $CLANG_VERSIONS; do
        echo "  build-time-insight analyze ${PREFIX}${version} --header-usage"
    done
}

if [ -n "$INTERNAL_BUILD" ]; then
    configure_and_build "$INTERNAL_BUILD"
elif [ -n "$BACKEND" ]; then
    if [ "$BACKEND" = nix ]; then
        build_versions_nix
    else
        build_versions_conda
    fi
    print_analysis_commands
else
    configure_and_build "$BUILD_DIR"
fi
