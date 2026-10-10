//! C++ build time analyzer.

use build_time_insight::report::{self, Report};
use build_time_insight::summary::ReportSummary;

use build_time_insight::{analyzer, build_info, cli};
use std::io::IsTerminal;

fn main() -> anyhow::Result<()> {
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
    let report = report::read_report(&args.report)?;
    let summary = ReportSummary::from_report(&report, args.limit());
    match &args.json {
        Some(path) => {
            let file = std::fs::File::create(path)?;
            serde_json::to_writer_pretty(std::io::BufWriter::new(file), &summary)?;
            println!("Wrote report summary to {}", path.display());
        }
        None => summary.print(),
    }
    Ok(())
}

fn run_compare(args: &cli::CompareArgs) -> anyhow::Result<()> {
    let base = report::read_report(&args.base)?;
    let target = report::read_report(&args.target)?;
    let diffs = report::unit_duration_diffs(&base, &target);
    println!("Per-unit total duration changes (positive = slower):");
    for (unit, diff) in &diffs {
        println!("{unit}: {diff} us");
    }
    Ok(())
}
