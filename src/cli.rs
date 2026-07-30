use std::env;
use std::ffi::OsString;
use std::path::PathBuf;

pub fn parse_dir_arg() -> Result<String, String> {
    validate_args(env::args_os())
}

fn validate_args<I>(args: I) -> Result<String, String>
where
    I: IntoIterator<Item = OsString>,
{
    let mut args_it = args.into_iter();
    args_it.next(); // Skip the executable name
    let arg = args_it
        .next()
        .ok_or_else(|| "Usage: build-time-insight /path/to/build/dir".to_string())?;
    let path = PathBuf::from(arg);
    if !path.exists() {
        return Err(format!(
            "Error: The path '{}' does not exist",
            path.display()
        ));
    }
    if !path.is_dir() {
        return Err(format!(
            "Error: The path '{}' is not a directory",
            path.display()
        ));
    }
    match path.into_os_string().into_string() {
        Ok(str) => Ok(str),
        Err(os_str) => Err(format!(
            "Error: The path '{}' does contains a valid string",
            os_str.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn test_missing_argument_returns_error() {
        let args = vec![OsString::from("my_executable")];
        let result = validate_args(args);
        assert!(result.is_err());
        assert_eq!(
            result.unwrap_err(),
            "Usage: build-time-insight /path/to/build/dir"
        )
    }

    #[test]
    fn test_non_existent_path_returns_error() {
        let args = vec![
            OsString::from("my_executable"),
            OsString::from("/test/invalid/path"),
        ];
        let result = validate_args(args);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("does not exist"));
    }

    #[test]
    fn test_valid_directory_succeeds() {
        let args = vec![OsString::from("my_executable"), OsString::from("src")];
        let result = validate_args(args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), PathBuf::from("src"));
    }
}
