//! C++ build time analyzer.

use build_time_insight::report::{self, Report};
use build_time_insight::summary::ReportSummary;

use build_time_insight::{analyzer, build_info, cli, compare};
use std::io::IsTerminal;

fn main() -> anyhow::Result<()> {
    reset_broken_pipe();
    let cli = cli::Cli::parse();
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(cli.log_level().to_string()),
    )
    .init();

    let quiet = cli.quiet;
    match cli.command {
        cli::Command::Analyze(args) => run_analyze(&args, quiet),
        cli::Command::Report(args) => run_report(&args),
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

fn run_compare(args: &cli::CompareArgs) -> anyhow::Result<()> {
    let base = report::read_report(&args.base)?;
    let target = report::read_report(&args.target)?;
    let diffs = compare::unit_duration_diffs(&base, &target);

    println!("Per-unit total duration changes (base -> target):");
    if diffs.is_empty() {
        println!("  (no translation units in either report)");
        return Ok(());
    }

    let unit_width = diffs
        .iter()
        .map(|diff| diff.unit.chars().count())
        .max()
        .unwrap_or(0);
    for diff in &diffs {
        println!("{:<unit_width$}  {}", diff.unit, diff.describe());
    }
    Ok(())
}
