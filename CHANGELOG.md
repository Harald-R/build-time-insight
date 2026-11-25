# Changelog

## 0.1.0 - 2026-11-25

Initial release.

### Added

- `analyze` command that reads Clang `-ftime-trace` files and writes a
  gzip-compressed MessagePack report.
- `report` command that prints build totals, ranked translation units,
  headers, and template instantiations, or writes the summary as YAML.
- `compare` command for diffing two reports, including configurable
  significance thresholds, filtered text output, YAML output, and a
  `--fail-on-regression` CI gate.
- Frontend, backend, other, and template-instantiation duration totals.
- Header parse-time analysis with inclusion counts and inclusion paths.
- Optional header usage analysis (`--header-usage`) that identifies unused
  includes, declaration-only includes, and includes that need definitions.
- Optional CMake/Ninja target dependency and compile-time analysis.
- Self-contained HTML dashboard in `web/report.html` with overview KPIs,
  Pareto charts, ranked tables, header usage details, target graphs, and
  header inclusion graphs.

### Notes

- The portable report format is not yet frozen. Future versions may not
  be able to parse reports generated with this version.
