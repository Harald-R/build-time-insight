mod analyzer;
mod build_traces;
mod cli;
mod raw_trace;

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

    let analysis_result = match build_traces::collect_build_traces(&build_dir) {
        Ok(traces) => analyzer::analyze(traces),
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
        println!(
            "{}. {}: {} us (included {} times)",
            header.0 + 1,
            header.1.0,
            header.1.1.parse_duration,
            header.1.1.num_inclusions
        );
    }
}
