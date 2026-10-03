# Fixture C++ project: `shapely`

A small-to-medium C++ project used as *input data* for testing the
`build-time-insight` analyzer. It is compiled with `-ftime-trace` enabled,
so the build directory gains one `<tu>.json` trace per translation unit; the
analyzer turns those traces into a report, and tests compare the report
against the expected values below.

## Layout

```
cpp-project/
├── CMakeLists.txt       # C++17; exports compile_commands.json; ENABLE_FTIME_TRACE=ON adds -ftime-trace
├── build-traces.sh      # configure + build (build dir inside the repo)
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

**8 translation units.** Include relationships create these inclusion chains:

- `geometry.cpp`                 -> `geometry.h` -> `vector3.h`
- `shapes.cpp`                   -> `shapes.h` -> `geometry.h` -> `vector3.h` (+ `utils.h`)
- `main.cpp`                     -> `geometry.h`, `shapes.h`, `utils.h`
- `src/edge/edge_unused.cpp`     -> `unused.h`, `geometry.h` -> `vector3.h`, `shapes.h`
- `src/edge/edge_fwd.cpp`        -> `fwd_class.h`, `geometry.h` -> `vector3.h`
- `src/edge/edge_needs.cpp`      -> `fwd_class.h`, `geometry.h` -> `vector3.h`
- `src/edge/edge_diamond.cpp`    -> `diamond_a.h`, `geometry.h` -> `vector3.h`
- `src/edge/edge_diamond2.cpp`   -> `diamond_b.h` -> `diamond_a.h`, `geometry.h` -> `vector3.h`

## Building the traces

```
./build-traces.sh [<dir>]    # By default, builds in `./build-traces`, unless a custom directory is provided
```

Requires `clang++` and `CMake`/`Ninja`. Produces `*.cpp.json` trace files
plus `compile_commands.json` (needed by `--header-usage`).

## Building with multiple clang versions (conda)

`--build-all` builds the fixture with every clang version. For each version it
creates the conda env `bti-clang<NN>` (clang, CMake, Ninja) if it does not
exist yet, then builds inside that env into `build-clang-<NN>`:

```
./build-traces.sh --build-all
./build-traces.sh --build-all myprefix-     # custom directory prefix
BTI_CLANG_VERSIONS="16 20" ./build-traces.sh --build-all   # custom versions
```

Or enter an environment and build a single version manually:

```
conda activate bti-clang18
./build-traces.sh build-clang18
build-time-insight analyze build-clang18 --header-usage
```

Notes:
- `--build-all` creates each env with `clang`, then runs the build inside the **activated**
  env so the C++ standard-library paths are set up.
- Keep the build directory inside the repository so `--header-usage` path
  relativization works (the analyzer's `project_root` runs `git rev-parse`).

## Building the traces

```
./build-traces.sh [<dir>]    # By default, builds in `./build-traces`, unless a custom directory is provided
```

Requires `clang++` and `CMake`/`Ninja`. Produces `*.cpp.json` trace files
plus `compile_commands.json` (needed by `--header-usage`).

## Building with multiple clang versions (conda)

`--build-all` builds the fixture with every clang version. For each version it
creates the conda env `bti-clang<NN>` (clang, CMake, Ninja) if it does not
exist yet, then builds inside that env:

```
./build-traces.sh --build-all
./build-traces.sh --build-all myprefix-     # custom directory prefix
BTI_CLANG_VERSIONS="16 20" ./build-traces.sh --build-all   # custom versions
```

Or enter an environment and build a single version manually:

```
conda activate bti-clang18
./build-traces.sh build-clang18
build-time-insight analyze build-clang18 --header-usage
```

Notes:
- `--build-all` prepends each env's `bin` dir to `PATH`, so cmake picks up that
  env's clang++, CMake and Ninja without `conda activate`.
- Keep the build directory inside the repository so `--header-usage` path
  relativization works (the analyzer's `project_root` runs `git rev-parse`).

## Analyze

```
build-time-insight analyze <build-dir> --header-usage
```
