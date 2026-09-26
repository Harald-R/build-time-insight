use std::path::PathBuf;

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

pub fn parse_dir_arg() -> String {
    Cli::parse().build_dir.to_string_lossy().into_owned()
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
}
