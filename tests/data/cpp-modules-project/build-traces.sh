#!/usr/bin/env bash
# Configure and build the C++20 modules fixture with clang's -ftime-trace.
#
# C++20 named modules require CMake >= 3.28 and a clang-scan-deps binary that
# matches the compiler. --nix supplies both from the same nixpkgs revision.
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
                       "18 19 20 21 22 23").
  --conda              Use conda environments.
  --nix                Use Nix. Each version is provided by nixpkgs.
  --backend <name>     Select a backend: "conda" or "nix".
  -h, --help           Show this help.

Environment:
  BTI_NIXPKGS          Override the nixpkgs flake ref used by --nix.

Examples:
  ./build-traces.sh --nix
  ./build-traces.sh --nix --versions "18 20 23" myprefix-
  ./build-traces.sh --conda --versions "18 20"

A single Nix-provided version can also be built manually:

  nix shell github:NixOS/nixpkgs/nixos-25.05#llvmPackages_18.clang \
      github:NixOS/nixpkgs/nixos-25.05#llvmPackages_18.clang-tools \
      github:NixOS/nixpkgs/nixos-25.05#cmake \
      github:NixOS/nixpkgs/nixos-25.05#ninja \
      -c ./build-traces.sh build-clang-18
EOF
}

CLANG_VERSIONS="18 19 20 21 22 23"
VERSIONS_SET=0
BACKEND=""
PREFIX="build-clang-"
BUILD_DIR="build-traces"
INTERNAL_BUILD=""

while [ $# -gt 0 ]; do
    case "$1" in
        --conda) BACKEND=conda ;;
        --nix) BACKEND=nix ;;
        --versions)
            if [ $# -lt 2 ]; then
                echo "error: --versions requires a value, e.g. \"18 20 23\"" >&2
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

configure_and_build() {
    local dir="$1" compiler scan_deps
    compiler="$(command -v clang++ || true)"
    if [ -z "$compiler" ] || [ ! -x "$compiler" ]; then
        echo "error: clang++ not found on PATH" >&2
        exit 1
    fi
    scan_deps="$(command -v clang-scan-deps || true)"
    if [ -z "$scan_deps" ] || [ ! -x "$scan_deps" ]; then
        echo "error: clang-scan-deps not found on PATH (C++20 modules need it)" >&2
        exit 1
    fi
    echo "  compiler: $compiler ($("$compiler" --version | head -1))"
    echo "  scanner:  $scan_deps ($("$scan_deps" --version | head -1))"
    cmake -S . -B "$dir" -G Ninja \
        -DCMAKE_CXX_COMPILER="$compiler" \
        -DCMAKE_CXX_COMPILER_CLANG_SCAN_DEPS="$scan_deps" \
        -DENABLE_FTIME_TRACE=ON
    cmake --build "$dir"
    echo
    echo "Trace files produced ($dir):"
    find "$dir" \( -name '*.cpp.json' -o -name '*.cppm.json' \) | sort
}

ensure_conda_env() {
    local version="$1" env_name="bti-modules-clang${1}"
    if conda env list | awk '{print $1}' | grep -qx "$env_name"; then
        return 0
    fi
    echo "==> Creating conda env '$env_name' (clang $version)"
    conda create -y -n "$env_name" -c conda-forge \
        "clang=${version}.*" "clangxx=${version}.*" "clang-tools=${version}.*" cmake ninja
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
        env_name="bti-modules-clang${version}"
        ensure_conda_env "$version"
        echo "==> Building with clang $version -> $dir (env $env_name)"
        (
            source "$base/etc/profile.d/conda.sh"
            conda activate "$env_name"
            configure_and_build "$dir"
        )
    done
}

nixpkgs_for_version() {
    local version="$1"
    if [ -n "${BTI_NIXPKGS:-}" ]; then
        printf '%s' "$BTI_NIXPKGS"
    elif [ "$version" -ge 22 ]; then
        printf 'github:NixOS/nixpkgs/nixpkgs-unstable'
    else
        printf 'github:NixOS/nixpkgs/nixos-25.05'
    fi
}

build_versions_nix() {
    local version major dir ref
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
        if [ "$major" -lt 18 ]; then
            echo "error: C++20 modules fixture requires clang 18 or newer (got $version)" >&2
            exit 1
        fi
        dir="${PREFIX}${version}"
        ref="$(nixpkgs_for_version "$major")"
        echo "==> Building with clang $version -> $dir ($ref)"
        nix shell \
            "${ref}#llvmPackages_${major}.clang" \
            "${ref}#llvmPackages_${major}.clang-tools" \
            "${ref}#cmake" \
            "${ref}#ninja" \
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
