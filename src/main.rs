use build_time_insight::report::{self, Report};

use build_time_insight::{analyzer, build_info, cli};

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(cli.log_level().to_string()),
    )
    .init();

    match cli.command {
        cli::Command::Analyze(args) => run_analyze(&args),
        cli::Command::Compare(args) => run_compare(&args),
    }
}

fn run_analyze(args: &cli::AnalyzeArgs) -> anyhow::Result<()> {
    let build_dir = args.build_dir.to_string_lossy().into_owned();
    let jobs = args.jobs();
    let limit = args.limit();
    log::info!("Build dir: {}", build_dir);
    log::info!("Jobs: {}", jobs);

    let build_info = build_info::extract_build_info(&build_dir);
    log::info!("Build info: {:#?}", build_info);

    let analysis_result = analyzer::analyze_build_dir(&build_dir, jobs, &args.exclude);

    println!("Longest files to compile:");
    for analysis in analysis_result.longest_unit_compilations(limit).enumerate() {
        println!(
            "{}. {}: {} us",
            analysis.0 + 1,
            analysis.1.0,
            analysis.1.1.total_duration
        );
    }

    println!("Longest headers to parse:");
    for header in analysis_result
        .longest_headers_parse_duration(limit)
        .enumerate()
    {
        let (name, analysis) = header.1;
        println!(
            "{}. {}: {} us (included {} times, avg {} us/inclusion)",
            header.0 + 1,
            name,
            analysis.parse_duration,
            analysis.num_inclusions,
            analysis.avg_inclusion_time()
        );
        let mut chains: Vec<(&String, &i64)> = analysis.inclusion_paths.iter().collect();
        chains.sort_by(|a, b| b.1.cmp(a.1));
        for (chain, count) in chains.into_iter().take(5) {
            println!("   via {} ({} times)", chain, count);
        }
    }

    println!("Largest template instantiations per translation unit:");
    for (source, analysis) in &analysis_result.unit_analysis {
        let function = analysis
            .largest_function_template_instantiation()
            .map(|(name, inst)| {
                format!(
                    "{} ({} us, {} instantiations, avg {} us)",
                    name, inst.total_duration, inst.num_instantiations, inst.avg_duration()
                )
            })
            .unwrap_or_else(|| "none".to_string());
        let class = analysis
            .largest_class_template_instantiation()
            .map(|(name, inst)| {
                format!(
                    "{} ({} us, {} instantiations, avg {} us)",
                    name, inst.total_duration, inst.num_instantiations, inst.avg_duration()
                )
            })
            .unwrap_or_else(|| "none".to_string());
        println!("{}: function: {}, class: {}", source, function, class);
    }

    println!("Largest template instantiations in the entire build:");
    for (rank, (kind, name, inst)) in analysis_result
        .longest_template_instantiations(limit)
        .enumerate()
    {
        println!(
            "{}. {} template '{}': {} us ({} instantiations, avg {} us)",
            rank + 1,
            kind,
            name,
            inst.total_duration,
            inst.num_instantiations,
            inst.avg_duration()
        );
    }

    let report = Report::new(build_info, analysis_result);
    let path = args
        .output
        .clone()
        .unwrap_or_else(|| std::env::temp_dir().join("report.msgpack"));
    report::write_report(&report, &path)?;
    log::info!("Wrote report to {}", path.display());
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
