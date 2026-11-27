# Fixture C++20 modules project

A C++20 named-modules project used as input data for `build-time-insight`'s
module support. It mixes module interface units, partitions, an implementation
unit, ordinary headers, STL headers, and exported templates.

## Layout

```
cpp-modules-project/
├── CMakeLists.txt
├── build-traces.sh
├── include/
│   ├── util.h                 # ordinary header used by modules and consumers
│   └── legacy.h               # plain header used next to module imports
└── src/
    ├── core.cppm              # primary module interface: export module core;
    ├── core-shapes.cppm       # partition interface: export module core:shapes;
    ├── core-detail.cppm       # internal partition: module core:detail;
    ├── core.cpp               # module implementation unit: module core;
    ├── main.cpp               # imports core; includes util.h/legacy.h
    └── consumer.cpp           # ordinary TU importing core and using STL headers
```

Covered features:

- primary module interface unit (`.cppm`)
- exported and internal module partitions
- module implementation unit (`.cpp`)
- `export import` re-export of a partition
- global module fragment with `<string>`, `<vector>`, and `<cmath>`
- exported class/function templates and a forced `Vec<double>` instantiation
- ordinary TUs that mix `import core;` with `#include "util.h"` and STL headers

## Requirements

- **C++20** (`-std=c++20`)
- **Clang 18 or newer** with C++20 modules support
- **CMake 3.28 or newer** for `CXX_MODULES` file sets
- **Ninja** with dyndep support
- **`clang-scan-deps` matching the compiler version** (CMake needs it to scan
  module dependencies)

Header units (`import "legacy.h";`, `import <vector>;`) and the C++23 standard
library modules (`import std;`, `import std.compat;`) are intentionally not part
of this baseline. Header units are experimental in Clang and need explicit BMI
generation; the STL module variant is handled separately.

## Building the traces

With a suitable `clang++` and `clang-scan-deps` on `PATH`:

```
./build-traces.sh [<dir>]    # default: ./build-traces
```

Or select a backend and version list:

```
./build-traces.sh --nix
./build-traces.sh --nix --versions "18 20 23" myprefix-
./build-traces.sh --conda --versions "18 20"
```

The Nix backend supplies the compiler, matching `clang-scan-deps`, CMake, and
Ninja from the same nixpkgs revision. Override it with `BTI_NIXPKGS` if needed.

Each build directory contains:

- `CMakeFiles/**/*.cpp.json` — ordinary and implementation-unit traces
- `CMakeFiles/**/*.cppm.json` — module interface and partition traces
- `compile_commands.json`
- `build.ninja`, `CMakeCache.txt`, and generated module metadata

CMake also emits `CXXModules.json` and `CXXDependInfo.json`; these are metadata,
not `-ftime-trace` output.

## Analyze

```
build-time-insight analyze build-clang-23 --header-usage
```
