# Build Time Insight

A command-line analyzer for Clang's `-ftime-trace` output. It aggregates the
data into a portable report, then lets you inspect or compare the build
performance without re-reading the traces.

The tool is intended to be useful in identifying build time hotspots.
It should also help with analyzing whether the build duration has improved
or regressed between two reports. With this in mind, the tool could also be
deployed in CI environments for tracking the build performance over time.

> Note: The portable report format is not yet frozen, as the tool is still in
> development. As such, there is no guarantee that the tool will be able to
> parse reports generated with older versions, as of now. The intention is to
> freeze this format once the tool reaches maturity.

## What it reports

- Build totals: total, frontend, backend, other, and template instantiation time
- Longest translation units to compile
- Largest function and class template instantiations
- Longest headers to parse, with inclusion counts and paths
- Optional header usage analysis: unused includes, declaration-only includes,
  and includes that require definitions
- Optional CMake target compile times and dependencies
- Per-translation-unit comparison between two builds

## How it works

1. `analyze` scans a build directory for Clang `-ftime-trace` files matching
   `**/*.cpp.json`. Each trace is parsed into an event tree and aggregated per
   translation unit, header, and template.
2. The result is written as a gzip-compressed MessagePack report.
3. `report` resolves the report and prints a summary or writes it as YAML.
4. `compare` matches items by resolved path/name across two reports, computes
   durations and deltas, and renders either text or canonical YAML.

`--header-usage` invokes the compiler recorded in `compile_commands.json` to
discover direct includes and walk the AST. The CMake target graph is read from
the CMake File API or the Ninja build file.

## Requirements

- A recent stable Rust toolchain (edition 2024)
- Clang, for producing traces with `-ftime-trace`
- `compile_commands.json` for `--header-usage` and for source-to-target mapping
- CMake File API output or a Ninja build file for target timings

## Build

```sh
cargo build --release
```

The binary is written to `target/release/build-time-insight`. To install it:

```sh
cargo install --path .
```

Run the test suite with:

```sh
cargo test
```

## Producing traces

Build the project with Clang's `-ftime-trace` flag. For CMake:

```sh
cmake -S . -B build -G Ninja -DCMAKE_CXX_FLAGS="-ftime-trace"
cmake --build build
```

Clang writes one JSON trace per translation unit, typically as
`<source>.cpp.json`, inside the build directory. The analyzer only reads
`*.cpp.json` files.

For meaningful comparisons, generate traces from clean builds. Incremental
builds only rewrite traces for translation units that were recompiled.

## Usage

### Analyze

```sh
# COMMAND: build-time-insight analyze <BUILD_DIR> [OPTIONS]

# Analyze the traces from the `build` directory and write the report to `report.bin`.
build-time-insight analyze build/ -o report.bin

# Analyze the traces from the `build` directory and limit the number of parallel
# worker threads to 4.
build-time-insight analyze build/ -j 4

# Analyze the traces from the `build` directory and classify the headers included as
# unused, declaration-only, or definition-required, using the compiler and flags from
# `compile_commands.json`.
# Note: This re-runs the compiler for every translation unit, so it is significantly
# slower than trace-only analysis. It also consumes more memory, so the number of
# parallel jobs may need to be limited.
build-time-insight analyze build/ --header-usage
```

### Report

```sh
# COMMAND: build-time-insight report <REPORT> [OPTIONS]

# Print the resolved summary: build info, totals, ranked units/headers/templates,
# header usage when available, and per-unit details.
build-time-insight report report.bin

# Show 50 entries in each list.
build-time-insight report report.bin --limit 50

# Write the resolved summary as YAML instead of printing it.
build-time-insight report report.bin --yaml summary.yaml
```

Example output, with paths shortened and some sections omitted:

```text
$ build-time-insight report report.bin --limit 3
Build: CMake 4.3.4, Ninja 1.13.2, Clang 21.1.8, ...

Totals
──────
  total: 993.766 ms
  frontend: 951.158 ms
  backend: 27.275 ms
  other: 15.333 ms
  template instantiation: 103.359 ms
  translation units: 8
  headers: 111

Longest files to compile (3 of 8)
─────────────────────────────────
214.749 ms (21.6%)  src/main.cpp
116.866 ms (11.8%)  src/edge/edge_diamond.cpp
113.636 ms (11.4%)  src/geometry.cpp
```

### HTML visualization

The repository includes a self-contained dashboard in `web/report.html`. It shows
totals, ranked units/headers/templates, header usage, inclusion graphs, and
target graphs.

The dashboard is published to [here](https://harald-r.github.io/build-time-insight).
Open the page and load a gzip-compressed `report.bin` with drag-and-drop or the
file picker. Loaded reports stay in the browser; no report data is uploaded.
`web/report.html` can also be opened directly for local use.

### Compare

```sh
# COMMAND: build-time-insight compare <BASE> [TARGET] [OPTIONS]

# Compare two reports as text. Totals are always shown; other sections show
# changed rows, and units also include unchanged rows until a duration
# threshold is set.
build-time-insight compare base.bin target.bin

# A change is significant when it clears both the absolute floor
# (--min-delta) and the relative floor (--min-percent), for example 1 ms
# and 1%. Defaults are 0 us and 0%.
#
# Header inclusion-count changes use --min-inclusions (default 1).
build-time-insight compare base.bin target.bin \
  --min-delta 1ms --min-percent 1

# Limit text output to one direction: --only regressions or improvements.
build-time-insight compare base.bin target.bin \
  --only regressions --min-delta 1ms --min-percent 1

# Show every row, including unchanged and below-threshold changes.
build-time-insight compare base.bin target.bin \
  --min-delta 1ms --min-percent 1 --all

# Limit each section to the 10 largest changes.
build-time-insight compare base.bin target.bin --limit 10

# Write canonical YAML, including the effective thresholds.
build-time-insight compare base.bin target.bin --format yaml > diff.yaml

# Render a saved comparison from a file or stdin.
build-time-insight compare diff.yaml
build-time-insight compare - < diff.yaml

# CI gate. Exits 0 when no significant regression is found, 1 when one is
# found, and 2 on runtime, argument, parse, or validation errors. Both
# individual rows and build totals are checked.
build-time-insight compare base.bin target.bin \
  --min-delta 1ms --min-percent 1 --fail-on-regression
```

Example output, with some sections omitted:

```text
$ build-time-insight compare base.bin target.bin --min-delta 1ms --min-percent 1
Report comparison (base -> target)
Summary: 2 regressions, 1 improvement, 0 added, 0 removed; total 8.120 s -> 8.560 s (+440.000 ms, +5.4%)

Build totals (base -> target)
─────────────────────────────
  total                   8.120 s -> 8.560 s  +440.000 ms (+5.4%)  regression
  frontend                7.400 s -> 7.700 s  +300.000 ms (+4.1%)  regression
  backend                 500.000 ms -> 500.000 ms (unchanged)
  other                   220.000 ms -> 360.000 ms  +140.000 ms (+63.6%)  regression
  template instantiation  1.100 s -> 1.150 s  +50.000 ms (+4.5%)  regression

Headers (base -> target)
────────────────────────
  include/geometry.h  91.200 ms -> 128.400 ms  +37.200 ms (+40.8%)  regression

Per-unit total durations (base -> target)
─────────────────────────────────────────
  src/main.cpp       2.100 s -> 2.460 s  +360.000 ms (+17.1%)  regression
  src/geometry.cpp   900.000 ms -> 850.000 ms  -50.000 ms (-5.6%)  improvement
```

## Typical workflow

```sh
# Baseline
git switch main
cmake -S . -B build -G Ninja -DCMAKE_CXX_FLAGS="-ftime-trace"
cmake --build build
build-time-insight analyze build -o base.bin

# Change
git switch my-branch
cmake --build build --clean-first
build-time-insight analyze build -o target.bin

# Inspect regressions
build-time-insight compare base.bin target.bin \
  --min-delta 1ms --min-percent 1
```

## Notes

- Compare reports use path/name matching. Keep the project root and path
  representation consistent between the two reports.
- `--header-usage` re-runs the compiler for every translation unit, so it is
  significantly slower than trace-only analysis.
- Slimming is applied at analyze time. Very small `--max-per-unit` or
  `--max-templates` values can hide data from later comparisons.
- The binary report is an internal, compressed format. Regenerate reports
  rather than editing them by hand.

## License

This project is licensed under the [MIT License](LICENSE).
