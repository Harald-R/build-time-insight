mod build_traces;
mod cli;

fn main() {
    env_logger::init();

    let build_dir = match cli::parse_dir_arg() {
        Ok(path) => path,
        Err(err_msg) => {
            eprintln!("Error: {}", err_msg);
            std::process::exit(1);
        }
    };
    println!("Build dir: {}", build_dir);

    let build_traces = build_traces::collect_build_traces(&build_dir);
    for trace in build_traces.unwrap() {
        trace.trace_tree.print_tree(0, 0);
    }
}
