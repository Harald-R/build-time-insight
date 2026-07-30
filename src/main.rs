mod cli;

fn main() {
    let build_dir = match cli::parse_dir_arg() {
        Ok(path) => path,
        Err(err_msg) => {
            eprintln!("Error: {}", err_msg);
            std::process::exit(1);
        }
    };
    println!("Build dir: {}", build_dir);
}
