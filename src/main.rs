use crate::report::Report;

mod analyzer;
mod build_info;
mod build_traces;
mod cli;
mod raw_trace;
mod report;

fn main() {
    env_logger::init();

    let build_dir = match cli::parse_dir_arg() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };
    println!("Build dir: {}", build_dir);

    let build_info = build_info::extract_build_info(&build_dir);
    println!("Build info: {:#?}", build_info);

    let analysis_result = match build_traces::collect_build_traces(&build_dir) {
        Ok(traces) => {
            for t in &traces {
                t.trace_tree.print_tree(0);
            }
            analyzer::analyze(traces)
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    };

    println!("Longest files to compile:");
    for analysis in analysis_result.longest_unit_compilations(10).enumerate() {
        println!(
            "{}. {}: {} us",
            analysis.0 + 1,
            analysis.1.0,
            analysis.1.1.total_duration
        );
    }

    println!("Longest headers to parse:");
    for header in analysis_result
        .longest_headers_parse_duration(10)
        .enumerate()
    {
        let (name, analysis) = header.1;
        println!(
            "{}. {}: {} us (included {} times)",
            header.0 + 1,
            name,
            analysis.parse_duration,
            analysis.num_inclusions
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
                    "{} ({} us, {} instantiations)",
                    name, inst.total_duration, inst.num_instantiations
                )
            })
            .unwrap_or_else(|| "none".to_string());
        let class = analysis
            .largest_class_template_instantiation()
            .map(|(name, inst)| {
                format!(
                    "{} ({} us, {} instantiations)",
                    name, inst.total_duration, inst.num_instantiations
                )
            })
            .unwrap_or_else(|| "none".to_string());
        println!("{}: function: {}, class: {}", source, function, class);
    }

    println!("Largest template instantiations in the entire build:");
    for (rank, (kind, name, inst)) in analysis_result
        .longest_template_instantiations(10)
        .enumerate()
    {
        println!(
            "{}. {} template '{}': {} us ({} instantiations)",
            rank + 1,
            kind,
            name,
            inst.total_duration,
            inst.num_instantiations
        );
    }

    let report = Report::new(build_info,  analysis_result);
    let path = std::env::temp_dir().join("report.msgpack");
    report::write_report(&report, &path).unwrap();
}
