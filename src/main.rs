//! C++ build time analyzer.

use anyhow::Context;
use build_time_insight::report::{self, Report};
use build_time_insight::summary::ReportSummary;

use build_time_insight::{analyzer, build_info, cli, compare};
use std::io::{IsTerminal, Read};
use std::process::ExitCode;

fn main() -> ExitCode {
    reset_broken_pipe();
    match run() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("Error: {err:?}");
            ExitCode::from(2)
        }
    }
}

fn run() -> anyhow::Result<ExitCode> {
    let cli = cli::Cli::parse();
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(cli.log_level().to_string()),
    )
    .init();

    let quiet = cli.quiet;
    match cli.command {
        cli::Command::Analyze(args) => {
            run_analyze(&args, quiet)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::Report(args) => {
            run_report(&args)?;
            Ok(ExitCode::SUCCESS)
        }
        cli::Command::Compare(args) => run_compare(&args),
    }
}

/// Restores the default SIGPIPE behavior on Unix.
///
/// Rust ignores it by default, so writing to a closed pipe (for example
/// `build-time-insight report report.bin | head`) makes `println!` panic.
/// Restoring the default disposition makes the process terminate quietly, as
/// most Unix command-line tools do.
#[cfg(unix)]
fn reset_broken_pipe() {
    // SAFETY: setting the default disposition for SIGPIPE is sound and the
    // call does not dereference any pointer.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

/// No-op on platforms without SIGPIPE.
#[cfg(not(unix))]
fn reset_broken_pipe() {}

fn run_analyze(args: &cli::AnalyzeArgs, quiet: bool) -> anyhow::Result<()> {
    let show_progress = !quiet && !args.no_progress && std::io::stderr().is_terminal();
    let build_dir = args.build_dir.to_string_lossy().into_owned();
    let jobs = args.jobs();
    log::info!("Build dir: {}", build_dir);
    log::info!("Jobs: {}", jobs);

    let build_info = build_info::extract_build_info(&build_dir);
    log::info!("Build info: {:#?}", build_info);

    let analysis = analyzer::analyze_build_dir(
        &build_dir,
        jobs,
        &args.exclude,
        args.header_usage,
        show_progress,
    )?;

    let mut report = Report::new(analysis.strings, build_info, analysis.result);
    report.slim(args.max_per_unit, args.max_templates);
    let path = &args.output;
    report::write_report(&report, path)?;
    println!("Wrote report to {}", path.display());
    Ok(())
}

fn run_report(args: &cli::ReportArgs) -> anyhow::Result<()> {
    let limit = args.limit();
    let report = report::read_report(&args.report)?;
    let summary = ReportSummary::from_report(&report, limit);
    match &args.json {
        Some(path) => {
            let file = std::fs::File::create(path)?;
            serde_json::to_writer_pretty(std::io::BufWriter::new(file), &summary)?;
            println!("Wrote report summary to {}", path.display());
        }
        None => summary.print(limit),
    }
    Ok(())
}

fn run_compare(args: &cli::CompareArgs) -> anyhow::Result<ExitCode> {
    let (diff, mut thresholds) = match &args.target {
        Some(target) => {
            let base = report::read_report(&args.base)?;
            let target = report::read_report(target)?;
            (
                compare::report_diff(&base, &target),
                compare::Thresholds::default(),
            )
        }
        None => {
            let document = read_yaml_comparison(&args.base)?;
            (document.diff, document.thresholds)
        }
    };
    apply_threshold_overrides(&mut thresholds, args);

    match args.format {
        cli::OutputFormat::Yaml => {
            print!("{}", compare::yaml::to_string(&diff, &thresholds)?);
        }
        cli::OutputFormat::Text => {
            diff.print_with(&render_options(args.all, args.only, args.limit, thresholds));
        }
    }

    if args.fail_on_regression && diff.has_significant_regression(&thresholds) {
        Ok(ExitCode::from(1))
    } else {
        Ok(ExitCode::SUCCESS)
    }
}

fn apply_threshold_overrides(thresholds: &mut compare::Thresholds, args: &cli::CompareArgs) {
    if let Some(min_delta) = args.min_delta {
        thresholds.min_delta_us = min_delta;
    }
    if let Some(min_percent) = args.min_percent {
        thresholds.min_percent = min_percent;
    }
    if let Some(min_inclusions) = args.min_inclusions {
        thresholds.min_inclusions = min_inclusions;
    }
}

/// Reads a YAML comparison from `path`, or from stdin when it is `-`.
fn read_yaml_comparison(
    path: &std::path::Path,
) -> anyhow::Result<compare::yaml::ComparisonDocument> {
    if path.to_str() == Some("-") {
        let mut input = String::new();
        std::io::stdin().read_to_string(&mut input)?;
        return Ok(compare::yaml::from_str(&input)?);
    }

    let bytes =
        std::fs::read(path).with_context(|| format!("failed to read '{}'", path.display()))?;
    if bytes.starts_with(&[0x1f, 0x8b]) {
        anyhow::bail!(
            "'{}' is a binary report; provide a target report to compare it against",
            path.display()
        );
    }
    let yaml = String::from_utf8(bytes)
        .with_context(|| format!("'{}' is not a UTF-8 YAML comparison", path.display()))?;
    Ok(compare::yaml::from_str(&yaml)?)
}

fn render_options(
    all: bool,
    only: Option<cli::ChangeFilter>,
    limit: Option<usize>,
    thresholds: compare::Thresholds,
) -> compare::RenderOptions {
    compare::RenderOptions {
        all,
        only: only.map(|only| match only {
            cli::ChangeFilter::Regressions => compare::Only::Regressions,
            cli::ChangeFilter::Improvements => compare::Only::Improvements,
        }),
        limit,
        thresholds,
    }
}
