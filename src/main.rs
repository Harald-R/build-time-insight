use build_time_insight::analyzer::HeaderAnalysis;
use build_time_insight::interner::StringId;
use build_time_insight::report::{self, Report};

use build_time_insight::{analyzer, build_info, cli};
use std::io::IsTerminal;

fn main() -> anyhow::Result<()> {
    let cli = cli::Cli::parse();
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or(cli.log_level().to_string()),
    )
    .init();

    let show_progress = !cli.quiet && std::io::stderr().is_terminal();
    match cli.command {
        cli::Command::Analyze(args) => run_analyze(&args, show_progress),
        cli::Command::Compare(args) => run_compare(&args),
    }
}

fn run_analyze(args: &cli::AnalyzeArgs, show_progress: bool) -> anyhow::Result<()> {
    let build_dir = args.build_dir.to_string_lossy().into_owned();
    let jobs = args.jobs();
    let limit = args.limit();
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
    );

    println!("Longest files to compile:");
    for item in analysis.result.longest_unit_compilations(limit).enumerate() {
        println!(
            "{}. {}: {} us",
            item.0 + 1,
            analysis.resolve(*item.1.0),
            item.1.1.total_duration
        );
    }

    println!("Longest headers to parse:");
    for header in analysis
        .result
        .longest_headers_parse_duration(limit)
        .enumerate()
    {
        let (name_id, header_analysis) = header.1;
        let name = analysis.resolve(*name_id);
        println!(
            "{}. {}: {} us (included {} times, avg {} us/inclusion)",
            header.0 + 1,
            name,
            header_analysis.parse_duration,
            header_analysis.num_inclusions,
            header_analysis.avg_inclusion_time()
        );
        let mut chains: Vec<(&Vec<StringId>, &i64)> =
            header_analysis.inclusion_paths.iter().collect();
        chains.sort_by(|a, b| b.1.cmp(a.1));
        for (chain, count) in chains.into_iter().take(5) {
            let resolved: Vec<String> = chain
                .iter()
                .map(|&id| analysis.resolve(id).to_string())
                .collect();
            let full_chain = if resolved.is_empty() {
                name.to_string()
            } else {
                format!("{} -> {}", resolved.join(" -> "), name)
            };
            println!("   via {full_chain} ({} times)", count);
        }
    }

    println!("Largest template instantiations per translation unit:");
    for (source_id, unit) in &analysis.result.unit_analysis {
        let function = unit
            .largest_function_template_instantiation()
            .map(|(name_id, inst)| {
                format!(
                    "{} ({} us, {} instantiations, avg {} us)",
                    analysis.resolve(*name_id),
                    inst.total_duration,
                    inst.num_instantiations,
                    inst.avg_duration()
                )
            })
            .unwrap_or_else(|| "none".to_string());
        let class = unit
            .largest_class_template_instantiation()
            .map(|(name_id, inst)| {
                format!(
                    "{} ({} us, {} instantiations, avg {} us)",
                    analysis.resolve(*name_id),
                    inst.total_duration,
                    inst.num_instantiations,
                    inst.avg_duration()
                )
            })
            .unwrap_or_else(|| "none".to_string());
        println!(
            "{}: function: {}, class: {}",
            analysis.resolve(*source_id),
            function,
            class
        );
    }

    println!("Largest template instantiations in the entire build:");
    for (rank, (kind, name_id, inst)) in analysis
        .result
        .longest_template_instantiations(limit)
        .enumerate()
    {
        println!(
            "{}. {} template '{}': {} us ({} instantiations, avg {} us)",
            rank + 1,
            kind,
            analysis.resolve(*name_id),
            inst.total_duration,
            inst.num_instantiations,
            inst.avg_duration()
        );
    }

    if args.header_usage {
        println!("Header usage:");
        let mut headers: Vec<(&StringId, &HeaderAnalysis)> = analysis
            .result
            .header_analysis
            .iter()
            .filter(|(_, ha)| {
                ha.unused_units + ha.forward_declarable_units + ha.definition_required_units > 0
            })
            .collect();
        headers.sort_by_key(|(_, ha)| {
            std::cmp::Reverse(ha.definition_required_units + ha.forward_declarable_units)
        });
        if headers.is_empty() {
            println!("  (none found; clang could not analyze the translation units)");
        }
        for (header_id, ha) in headers {
            let header = analysis.resolve(*header_id);
            if ha.unused_units > 0 {
                println!(
                    "  {}: unused in {} unit(s) -> remove #include",
                    header, ha.unused_units
                );
            }
            if ha.forward_declarable_units > 0 {
                println!(
                    "  {}: replace #include with forward declaration in {} unit(s)",
                    header, ha.forward_declarable_units
                );
            }
            if ha.definition_required_units > 0 {
                println!(
                    "  {}: needs definition in {} unit(s)",
                    header, ha.definition_required_units
                );
            }
        }
    }

    let report = Report::new(analysis.strings, build_info, analysis.result);
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
