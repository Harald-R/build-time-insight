use std::path::PathBuf;
use std::thread;

use clap::Parser;

#[derive(Debug, Parser)]
#[command(
    version,
    about = "Analyze clang -ftime-trace output to find C++ build-time bottlenecks"
)]
pub struct Cli {
    /// Build directory containing the clang `-ftime-trace` JSON files
    #[arg(value_parser = validate_build_dir)]
    pub build_dir: PathBuf,

    /// Number of parallel worker threads to use for the analysis
    #[arg(short, long, value_parser = parse_jobs)]
    pub jobs: Option<usize>,
}

impl Cli {
    pub fn parse() -> Self {
        <Self as Parser>::parse()
    }

    /// Resolve the number of worker threads, defaulting to all available CPUs
    pub fn jobs(&self) -> usize {
        self.jobs.unwrap_or_else(default_jobs)
    }
}

pub fn default_jobs() -> usize {
    thread::available_parallelism().map_or(1, |n| n.get())
}

fn parse_jobs(s: &str) -> Result<usize, String> {
    let n: usize = s
        .parse()
        .map_err(|_| format!("'{s}' is not a valid number of jobs"))?;
    if n == 0 {
        return Err("number of jobs must be at least 1".to_string());
    }
    Ok(n)
}

/// Validates that a build directory argument exists and is a directory
fn validate_build_dir(s: &str) -> Result<PathBuf, String> {
    let path = PathBuf::from(s);
    if !path.exists() {
        return Err(format!("The path '{}' does not exist", path.display()));
    }
    if !path.is_dir() {
        return Err(format!("The path '{}' is not a directory", path.display()));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_missing_argument_returns_error() {
        let err = Cli::try_parse_from(["build-time-insight"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
    }

    #[test]
    fn test_non_existent_path_returns_error() {
        let err = Cli::try_parse_from(["build-time-insight", "/test/invalid/path"]).unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn test_valid_directory_succeeds() {
        let cli = Cli::try_parse_from(["build-time-insight", "src"]).unwrap();
        assert_eq!(cli.build_dir, PathBuf::from("src"));
    }

    #[test]
    fn test_jobs_argument_controls_thread_count() {
        let cli = Cli::try_parse_from(["build-time-insight", "src", "-j", "2"]).unwrap();
        assert_eq!(cli.jobs(), 2);
    }

    #[test]
    fn test_jobs_defaults_to_all_available_threads() {
        let cli = Cli::try_parse_from(["build-time-insight", "src"]).unwrap();
        assert_eq!(cli.jobs(), default_jobs());
    }

    #[test]
    fn test_jobs_rejects_zero() {
        let err = Cli::try_parse_from(["build-time-insight", "src", "-j", "0"]).unwrap_err();
        assert_eq!(err.kind(), clap::error::ErrorKind::ValueValidation);
    }
}
