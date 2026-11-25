//! Provides the command-line interface.

use std::path::PathBuf;
use std::thread;

use clap::{Args, Parser, Subcommand, ValueEnum};

/// Default path the `analyze` command writes its report to.
pub const DEFAULT_REPORT_PATH: &str = "report.bin";

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Analyze clang -ftime-trace output to find C++ build-time bottlenecks"
)]
/// Defines the command-line arguments for the application.
pub struct Cli {
    #[command(subcommand)]
    /// Selects the command to run.
    pub command: Command,

    /// Increase logging verbosity; repeat for more detail.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,

    /// Only log errors.
    #[arg(short, long, global = true)]
    pub quiet: bool,
}

/// Defines the commands that the tool can run.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Analyze a build directory and write a portable report.
    Analyze(AnalyzeArgs),

    /// Read a report and print its results, or write them to YAML.
    Report(ReportArgs),

    /// Compare two reports, or render a saved comparison, as text or YAML.
    Compare(CompareArgs),
}

/// Defines the arguments for the analyze command.
#[derive(Debug, Args)]
pub struct AnalyzeArgs {
    /// Build directory containing the clang `-ftime-trace` JSON files.
    #[arg(value_parser = validate_build_dir)]
    pub build_dir: PathBuf,

    /// Number of parallel worker threads to use for the analysis.
    #[arg(short, long, value_parser = parse_jobs)]
    pub jobs: Option<usize>,

    /// Path where the report is written.
    #[arg(short, long, default_value = DEFAULT_REPORT_PATH)]
    pub output: PathBuf,

    /// Glob patterns of trace files to exclude from the analysis; may be repeated.
    #[arg(short, long)]
    pub exclude: Vec<String>,

    /// Classify directly included project headers as unused, declaration-only,
    /// or definition-required, using the compiler and flags from
    /// `compile_commands.json`.
    ///
    /// This re-runs the compiler for every translation unit, so it is
    /// significantly slower than trace-only analysis and consumes more memory.
    /// Reduce `--jobs` if needed.
    #[arg(long)]
    pub header_usage: bool,

    /// Largest number of headers and templates kept for each translation unit.
    /// Use 0 to keep all.
    #[arg(long, default_value_t = 50)]
    pub max_per_unit: usize,

    /// Largest number of templates kept for the whole build. Use 0 to keep all.
    #[arg(long, default_value_t = 5000)]
    pub max_templates: usize,

    /// Disable the progress bar.
    #[arg(long)]
    pub no_progress: bool,
}

/// Defines the arguments for the report command.
#[derive(Debug, Args)]
pub struct ReportArgs {
    /// Report file to read.
    #[arg(value_parser = validate_report_file)]
    pub report: PathBuf,

    /// Number of entries to show in each top-N list.
    #[arg(short, long, value_parser = parse_limit)]
    pub limit: Option<usize>,

    /// Write the results to this YAML file instead of printing them.
    #[arg(long)]
    pub yaml: Option<PathBuf>,
}

/// The format written to stdout by the compare command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Human-readable text.
    Text,
    /// Canonical machine-readable YAML.
    Yaml,
}

/// Which changes to keep when rendering text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum ChangeFilter {
    /// Slower and added rows.
    Regressions,
    /// Faster and removed rows.
    Improvements,
}

/// Defines the arguments for the compare command.
#[derive(Debug, Args)]
pub struct CompareArgs {
    /// Baseline report file, or a YAML comparison when TARGET is omitted.
    pub base: PathBuf,

    /// Report file to compare against the baseline.
    ///
    /// Omit this when BASE is a YAML comparison or '-' for stdin.
    pub target: Option<PathBuf>,

    /// Format written to stdout.
    #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
    pub format: OutputFormat,

    /// Include unchanged rows in the text output.
    #[arg(long)]
    pub all: bool,

    /// Show only regressions or improvements in the text output.
    #[arg(long, value_enum)]
    pub only: Option<ChangeFilter>,

    /// Maximum number of rows printed per section.
    #[arg(short, long, value_parser = parse_limit)]
    pub limit: Option<usize>,

    /// Ignore duration changes smaller than this, e.g. 500us, 1ms, or 2s.
    /// Defaults to 0.
    #[arg(long, value_name = "DURATION", value_parser = parse_duration)]
    pub min_delta: Option<i64>,

    /// Ignore changes smaller than this percentage, e.g. 1 or 0.5.
    /// Defaults to 0.
    #[arg(long, value_name = "PERCENT", value_parser = parse_percent)]
    pub min_percent: Option<f64>,

    /// Ignore inclusion-count changes smaller than this. Defaults to 1.
    #[arg(long, value_name = "COUNT", value_parser = parse_count)]
    pub min_inclusions: Option<i64>,

    /// Exit with status 1 when a significant regression is found.
    ///
    /// A regression is significant when it clears both the absolute floor
    /// (`--min-delta`) and the relative floor (`--min-percent`). Header
    /// inclusion-count changes use `--min-inclusions` instead. Build totals
    /// are checked alongside individual rows.
    #[arg(long)]
    pub fail_on_regression: bool,
}

impl Cli {
    /// Parses the command-line arguments.
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }

    /// Resolve the log filter level from the verbosity and quiet flags.
    pub fn log_level(&self) -> log::LevelFilter {
        if self.quiet {
            log::LevelFilter::Error
        } else {
            match self.verbose {
                0 => log::LevelFilter::Info,
                1 => log::LevelFilter::Debug,
                _ => log::LevelFilter::Trace,
            }
        }
    }
}

impl AnalyzeArgs {
    /// Resolve the number of worker threads, defaulting to the number of
    /// available system threads and never exceeding it.
    pub fn jobs(&self) -> usize {
        let system = default_jobs();
        self.jobs.unwrap_or(system).min(system)
    }
}

impl ReportArgs {
    /// Resolve the number of entries per top-N list.
    pub fn limit(&self) -> usize {
        self.limit.unwrap_or(25)
    }
}

/// Returns the number of available system threads.
pub fn default_jobs() -> usize {
    thread::available_parallelism().map_or(1, |n| n.get())
}

fn parse_jobs(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid number of jobs"))?;
    if n == 0 {
        return Err("number of jobs must be at least 1".to_string());
    }
    Ok(n)
}

fn parse_limit(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid limit"))?;
    if n == 0 {
        return Err("limit must be at least 1".to_string());
    }
    Ok(n)
}

fn parse_duration(s: &str) -> Result<i64, String> {
    let s = s.trim();
    let (value, multiplier) = if let Some(value) = s.strip_suffix("ms") {
        (value, 1_000.0)
    } else if let Some(value) = s.strip_suffix("us").or_else(|| s.strip_suffix("µs")) {
        (value, 1.0)
    } else if let Some(value) = s.strip_suffix('s') {
        (value, 1_000_000.0)
    } else {
        (s, 1.0)
    };
    let value: f64 = value
        .trim()
        .parse()
        .map_err(|_| format!("'{s}' is not a valid duration"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("'{s}' is not a valid duration"));
    }
    let micros = value * multiplier;
    if micros > i64::MAX as f64 {
        return Err(format!("'{s}' is too large"));
    }
    Ok(micros.round() as i64)
}

fn parse_percent(s: &str) -> Result<f64, String> {
    let value: f64 = s
        .trim()
        .trim_end_matches('%')
        .trim()
        .parse()
        .map_err(|_| format!("'{s}' is not a valid percentage"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(format!("'{s}' is not a valid percentage"));
    }
    Ok(value)
}

fn parse_count(s: &str) -> Result<i64, String> {
    let count: i64 = s
        .trim()
        .parse()
        .map_err(|_| format!("'{s}' is not a valid count"))?;
    if count < 0 {
        return Err("count must not be negative".to_string());
    }
    Ok(count)
}

/// Validates that a build directory argument exists and is a directory
fn validate_build_dir(s: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(s);
    if !path.exists() {
        return Err(format!("The path '{}' does not exist", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("The path '{}' is not a directory", path.display()));
    }
    Ok(path)
}

/// Validates that a report file argument exists and is a file.
fn validate_report_file(s: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(s);
    if !path.exists() {
        return Err(format!("The report '{}' does not exist", path.display()));
    }
    if !path.is_file() {
        return Err(format!("The report '{}' is not a file", path.display()));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn analyze_args(cli: &Cli) -> &AnalyzeArgs {
        match &cli.command {
            Command::Analyze(args) => args,
            _ => panic!("expected Analyze subcommand"),
        }
    }

    fn report_args(cli: &Cli) -> &ReportArgs {
        match &cli.command {
            Command::Report(args) => args,
            _ => panic!("expected Report subcommand"),
        }
    }

    #[test]
    fn test_missing_command_returns_error() {
        let err = Cli::try_parse_from(["build-time-insight"]).unwrap_err();
        assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
        );
    }

    #[test]
    fn test_non_existent_path_returns_error() {
        let err = Cli::try_parse_from(["build-time-insight", "analyze", "/test/invalid/path"])
            .unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn test_analyze_valid_directory_succeeds() {
        let cli = Cli::try_parse_from(["build-time-insight", "analyze", "src"]).unwrap();
        assert_eq!(analyze_args(&cli).build_dir, PathBuf::from("src"));
    }

    #[test]
    fn test_jobs_argument_controls_thread_count() {
        let cli = Cli::try_parse_from(["build-time-insight", "analyze", "src", "-j", "2"]).unwrap();
        assert_eq!(analyze_args(&cli).jobs(), 2);
    }

    #[test]
    fn test_jobs_defaults_to_all_available_threads() {
        let cli = Cli::try_parse_from(["build-time-insight", "analyze", "src"]).unwrap();
        assert_eq!(analyze_args(&cli).jobs(), default_jobs());
    }

    #[test]
    fn test_jobs_is_capped_at_available_threads() {
        let cli =
            Cli::try_parse_from(["build-time-insight", "analyze", "src", "-j", "999999"]).unwrap();
        assert_eq!(analyze_args(&cli).jobs(), default_jobs());
    }

    #[test]
    fn test_jobs_rejects_zero() {
        let err =
            Cli::try_parse_from(["build-time-insight", "analyze", "src", "-j", "0"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn test_output_argument_sets_report_path() {
        let cli = Cli::try_parse_from(["build-time-insight", "analyze", "src", "-o", "/tmp/r.bin"])
            .unwrap();
        assert_eq!(analyze_args(&cli).output, PathBuf::from("/tmp/r.bin"));
    }

    #[test]
    fn test_output_defaults_to_report_bin() {
        let cli = Cli::try_parse_from(["build-time-insight", "analyze", "src"]).unwrap();
        assert_eq!(
            analyze_args(&cli).output,
            PathBuf::from(DEFAULT_REPORT_PATH)
        );
    }

    #[test]
    fn test_limit_argument_controls_top_n() {
        let cli =
            Cli::try_parse_from(["build-time-insight", "report", "Cargo.toml", "--limit", "5"])
                .unwrap();
        assert_eq!(report_args(&cli).limit(), 5);
    }

    #[test]
    fn test_limit_defaults_to_25() {
        let cli = Cli::try_parse_from(["build-time-insight", "report", "Cargo.toml"]).unwrap();
        assert_eq!(report_args(&cli).limit(), 25);
    }

    #[test]
    fn test_limit_rejects_zero() {
        let err =
            Cli::try_parse_from(["build-time-insight", "report", "Cargo.toml", "--limit", "0"])
                .unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }

    #[test]
    fn test_report_command_parses_file_and_yaml() {
        let cli = Cli::try_parse_from([
            "build-time-insight",
            "report",
            "Cargo.toml",
            "--yaml",
            "/tmp/summary.yaml",
        ])
        .unwrap();
        let args = report_args(&cli);
        assert_eq!(args.report, PathBuf::from("Cargo.toml"));
        assert_eq!(args.yaml, Some(PathBuf::from("/tmp/summary.yaml")));
    }

    #[test]
    fn test_report_missing_file_returns_error() {
        let err = Cli::try_parse_from(["build-time-insight", "report", "/no/such/report.bin"])
            .unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn test_verbose_counts_occurrences() {
        let cli = Cli::try_parse_from(["build-time-insight", "analyze", "src", "-vv"]).unwrap();
        assert_eq!(cli.verbose, 2);
        assert_eq!(cli.log_level(), log::LevelFilter::Trace);
    }

    #[test]
    fn test_quiet_sets_error_level() {
        let cli = Cli::try_parse_from(["build-time-insight", "analyze", "src", "-q"]).unwrap();
        assert!(cli.quiet);
        assert_eq!(cli.log_level(), log::LevelFilter::Error);
    }

    #[test]
    fn test_exclude_accepts_multiple_patterns() {
        let cli = Cli::try_parse_from([
            "build-time-insight",
            "analyze",
            "src",
            "-e",
            "*tests*",
            "-e",
            "*third*",
        ])
        .unwrap();
        assert_eq!(
            analyze_args(&cli).exclude,
            vec!["*tests*".to_string(), "*third*".to_string()]
        );
    }

    #[test]
    fn test_compare_accepts_two_reports() {
        let cli = Cli::try_parse_from(["build-time-insight", "compare", "base.bin", "target.bin"])
            .unwrap();
        let args = match &cli.command {
            Command::Compare(args) => args,
            _ => panic!("expected Compare subcommand"),
        };
        assert_eq!(args.base, PathBuf::from("base.bin"));
        assert_eq!(args.target, Some(PathBuf::from("target.bin")));
        assert_eq!(args.format, OutputFormat::Text);
        assert!(!args.all);
        assert_eq!(args.only, None);
        assert_eq!(args.limit, None);
        assert_eq!(args.min_delta, None);
        assert_eq!(args.min_percent, None);
        assert_eq!(args.min_inclusions, None);
        assert!(!args.fail_on_regression);
    }

    #[test]
    fn test_compare_parses_thresholds_and_fail_flag() {
        let cli = Cli::try_parse_from([
            "build-time-insight",
            "compare",
            "base.bin",
            "target.bin",
            "--min-delta",
            "1.5ms",
            "--min-percent",
            "1.5%",
            "--min-inclusions",
            "3",
            "--fail-on-regression",
        ])
        .unwrap();
        let args = match &cli.command {
            Command::Compare(args) => args,
            _ => panic!("expected Compare subcommand"),
        };
        assert_eq!(args.min_delta, Some(1500));
        assert_eq!(args.min_percent, Some(1.5));
        assert_eq!(args.min_inclusions, Some(3));
        assert!(args.fail_on_regression);
    }

    #[test]
    fn test_compare_parses_duration_units() {
        for (input, expected) in [
            ("500us", 500),
            ("1ms", 1_000),
            ("2s", 2_000_000),
            ("250", 250),
            ("1.5ms", 1_500),
        ] {
            let cli = Cli::try_parse_from([
                "build-time-insight",
                "compare",
                "base.bin",
                "target.bin",
                "--min-delta",
                input,
            ])
            .unwrap();
            let args = match &cli.command {
                Command::Compare(args) => args,
                _ => panic!("expected Compare subcommand"),
            };
            assert_eq!(args.min_delta, Some(expected), "input: {input}");
        }
    }

    #[test]
    fn test_compare_rejects_negative_thresholds() {
        for flag in ["--min-delta=-1", "--min-percent=-1", "--min-inclusions=-1"] {
            let err = Cli::try_parse_from([
                "build-time-insight",
                "compare",
                "base.bin",
                "target.bin",
                flag,
            ])
            .unwrap_err();
            assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
        }
    }

    #[test]
    fn test_compare_accepts_single_yaml_input() {
        let cli = Cli::try_parse_from(["build-time-insight", "compare", "diff.yaml"]).unwrap();
        let args = match &cli.command {
            Command::Compare(args) => args,
            _ => panic!("expected Compare subcommand"),
        };
        assert_eq!(args.base, PathBuf::from("diff.yaml"));
        assert_eq!(args.target, None);
    }

    #[test]
    fn test_compare_accepts_yaml_and_render_options() {
        let cli = Cli::try_parse_from([
            "build-time-insight",
            "compare",
            "base.bin",
            "target.bin",
            "--format",
            "yaml",
            "--all",
            "--only",
            "regressions",
            "--limit",
            "5",
        ])
        .unwrap();
        let args = match &cli.command {
            Command::Compare(args) => args,
            _ => panic!("expected Compare subcommand"),
        };
        assert_eq!(args.format, OutputFormat::Yaml);
        assert!(args.all);
        assert_eq!(args.only, Some(ChangeFilter::Regressions));
        assert_eq!(args.limit, Some(5));
    }

    #[test]
    fn test_compare_single_input_accepts_view_options_and_stdin() {
        let cli = Cli::try_parse_from([
            "build-time-insight",
            "compare",
            "-",
            "--only",
            "improvements",
            "--limit",
            "10",
        ])
        .unwrap();
        let args = match &cli.command {
            Command::Compare(args) => args,
            _ => panic!("expected Compare subcommand"),
        };
        assert_eq!(args.base, PathBuf::from("-"));
        assert_eq!(args.target, None);
        assert_eq!(args.only, Some(ChangeFilter::Improvements));
        assert_eq!(args.limit, Some(10));
        assert!(!args.all);
    }

    #[test]
    fn test_header_usage_flag() {
        let cli = Cli::try_parse_from(["build-time-insight", "analyze", "src", "--header-usage"])
            .unwrap();
        let args = match &cli.command {
            Command::Analyze(args) => args,
            _ => panic!("expected Analyze subcommand"),
        };
        assert!(args.header_usage);
    }

    #[test]
    fn test_no_progress_flag() {
        let cli =
            Cli::try_parse_from(["build-time-insight", "analyze", "src", "--no-progress"]).unwrap();
        let args = match &cli.command {
            Command::Analyze(args) => args,
            _ => panic!("expected Analyze subcommand"),
        };
        assert!(args.no_progress);
    }
}
