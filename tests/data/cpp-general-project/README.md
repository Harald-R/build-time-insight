# Fixture C++ project: `shapely`

A small-to-medium C++ project used as *input data* for testing the
`build-time-insight` analyzer. It is compiled with `-ftime-trace` enabled,
so the build directory gains one `<tu>.json` trace per translation unit; the
analyzer turns those traces into a report, and tests compare the report
against the expected values below.

## Layout

```
cpp-general-project/
├── CMakeLists.txt       # C++17; exports compile_commands.json; ENABLE_FTIME_TRACE=ON adds -ftime-trace
├── build-traces.sh      # configure + build (conda or Nix)
├── include/
│   ├── vector3.h        # class template Vector3<T> (instantiates in many units)
│   ├── geometry.h       # non-template declarations; includes vector3.h
│   ├── shapes.h         # structs + declarations; includes geometry.h
│   ├── utils.h          # free function templates + an inline helper
│   ├── unused.h         # probe: included but never used       -> "Unused"
│   ├── fwd_class.h      # probe: class Widget + declaration    -> "NeedsDefinition"/"DeclarationOnly"
│   ├── diamond_a.h      # probe: reached directly (diamond pair)
│   └── diamond_b.h      # probe: includes diamond_a.h (indirect path)
└── src/
    ├── geometry.cpp     # implements geometry.h
    ├── shapes.cpp       # implements shapes.h
    ├── main.cpp         # driver; instantiates the project templates
```

## Building the traces

```
./build-traces.sh [<dir>]    # By default, builds in `./build-traces`, unless a custom directory is provided
```

Requires `clang++` and `CMake`/`Ninja`. Produces `*.cpp.json` trace files
plus `compile_commands.json` (needed by `--header-usage`).

## Building with multiple clang versions

Selecting a backend with `--conda` or `--nix` builds the fixture with every
version in the `--versions` list (default `21.1.8 22.1.8`). Each build goes
into `<prefix><version>` (default prefix `build-clang-`). Without a backend the
script only performs a single build with the `clang++` on `PATH`.

```
./build-traces.sh --conda
./build-traces.sh --nix
./build-traces.sh --conda myprefix-     # custom directory prefix
./build-traces.sh --conda --versions "16 20"   # custom versions
```

### Conda

For each version the script creates the conda env `bti-clang<version>` (clang,
CMake, Ninja) if it does not exist yet, then builds inside that env:

```
./build-traces.sh --conda
./build-traces.sh --conda --versions "16 20"
```

Notes:
- `--conda` creates each env with `clang`, then runs the build inside the
  **activated** env so the C++ standard-library paths are set up.
- Keep the build directory inside the repository so `--header-usage` path
  relativization works (the analyzer's `project_root` runs `git rev-parse`).

### Nix / NixOS

On NixOS (or any system with Nix flakes enabled), pass `--nix` to use clang,
CMake and Ninja from nixpkgs instead of conda:

```
./build-traces.sh --nix
./build-traces.sh --nix myprefix-
./build-traces.sh --nix --versions "18 20 22"
```

For each version the script runs the build inside:

```
nix shell <nixpkgs>#llvmPackages_<major>.clang <nixpkgs>#cmake <nixpkgs>#ninja
```

Versions may be full (`21.1.8`) or major-only (`21`); nixpkgs selects the patch
release it provides for that major.

nixpkgs only keeps the LLVM versions it still maintains; current nixpkgs
provides LLVM 18 and newer. To build every clang from 9 onward:

```
BTI_NIXPKGS=github:NixOS/nixpkgs/nixos-23.11 \
    ./build-traces.sh --nix --versions "9 10 11"

BTI_NIXPKGS=github:NixOS/nixpkgs/nixos-24.05 \
    ./build-traces.sh --nix --versions "12 13 14 15 16 17"

BTI_NIXPKGS=github:NixOS/nixpkgs/nixos-25.05 \
    ./build-traces.sh --nix --versions "18 19 20 21"

BTI_NIXPKGS=github:NixOS/nixpkgs/nixpkgs-unstable \
    ./build-traces.sh --nix --versions "22 23"
```

Notes:
- `--conda` and `--nix` (or `--backend conda|nix`) select the backend and
  trigger a multi-version build. One of them is required for that mode.
- Keep the build directory inside the repository so `--header-usage` path
  relativization works (the analyzer's `project_root` runs `git rev-parse`).

## Analyze

```
build-time-insight analyze <build-dir> --header-usage
```
