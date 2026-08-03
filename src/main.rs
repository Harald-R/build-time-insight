use crate::analyzer::analyze;

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

    match build_traces::collect_build_traces(&build_dir) {
        Ok(traces) => analyze(traces),
        Err(e) => {
            eprintln!("Error: {}", e);
            std::process::exit(1);
        }
    }
}
