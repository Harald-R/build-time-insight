//! Provides the build information of the project.

use std::{
    fs, io,
    path::{Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};

/// Describes a build tool.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Tool {
    /// Name of the tool.
    pub name: String,
    /// Version of the tool.
    pub version: String,
}

/// Describes the build toolchain.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Toolchain {
    /// Build system of the project (e.g. CMake).
    pub build_system: Option<Tool>,
    /// Generator of the build (e.g. Ninja).
    pub generator: Option<Tool>,
    /// C++ compiler of the build (e.g. clang++).
    pub cxx_compiler: Option<Tool>,
    /// C compiler of the build (e.g. clang).
    pub c_compiler: Option<Tool>,
    /// Linker of the build (e.g. lld).
    pub linker: Option<Tool>,
}

/// Describes the build information.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct BuildInfo {
    /// Toolchain of the build.
    pub toolchain: Option<Toolchain>,
    /// Git commit of the build.
    pub git_commit: Option<String>,
}

fn extract_version(tool_path: &str) -> String {
    Command::new(tool_path)
        .arg("--version")
        .output()
        .ok()
        .filter(|out| out.status.success())
        .and_then(|out| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .next()
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_else(|| {
            log::warn!("Running '{tool_path}' failed or could not start");
            String::new()
        })
}

fn get_cmake_cache_content(build_dir: &str) -> Result<String, io::Error> {
    let cmake_cache_path = build_dir.to_string() + "/CMakeCache.txt";
    fs::read_to_string(cmake_cache_path)
}

fn get_cmake_files_dir(build_dir: &str, toolchain: &Toolchain) -> Option<PathBuf> {
    log::trace!("Searching for CMakeFiles directory");
    let cmake_files_dir = build_dir.to_string() + "/CMakeFiles";
    let entries = fs::read_dir(cmake_files_dir).ok()?;

    let version_dirs: Vec<PathBuf> = entries
        .filter_map(|entry| {
            let path = entry.ok()?.path();
            let file_name = path.file_name()?.to_str()?;

            // Check if the directory name looks like a version (e.g. starts with a digit)
            if path.is_dir() && file_name.chars().next()?.is_ascii_digit() {
                Some(path)
            } else {
                None
            }
        })
        .collect();

    let found_versions: Vec<String> = version_dirs
        .iter()
        .filter_map(|path| {
            path.file_name()
                .map(|os_str| os_str.to_string_lossy().into_owned())
        })
        .collect();
    log::trace!(
        "Found {} CMakeFiles version directories: {:?}",
        version_dirs.len(),
        found_versions
    );

    select_cmake_files_dir(&version_dirs, toolchain)
}

fn select_cmake_files_dir(version_dirs: &[PathBuf], toolchain: &Toolchain) -> Option<PathBuf> {
    if let Some(build_system) = &toolchain.build_system {
        let found_dir = version_dirs
            .iter()
            .find(|path| {
                path.file_name()
                    .and_then(|os_str| os_str.to_str())
                    .map(|name| name.contains(&build_system.version))
                    .unwrap_or(false)
            })
            .cloned();
        if found_dir.is_some() {
            log::trace!(
                "Found CMakeFiles directory corresponding to version {}",
                build_system.version
            );
        } else {
            log::trace!(
                "Could not find CMakeFiles directory corresponding to version {}",
                build_system.version
            );
        }
        return found_dir;
    } else {
        log::warn!(
            "The CMake version is unknown. Searching for toolchain info in latest version's CMakeFiles"
        )
    };

    log::trace!("Using the latest CMake Files version");
    let mut sorted = version_dirs.to_vec();
    // Sort alphabetically / numerically so the latest version is at the end
    sorted.sort();
    sorted.pop()
}

fn parse_cmake_cache(cache_content: &str) -> Toolchain {
    log::trace!("Parsing CMake cache");
    let mut toolchain = Toolchain::default();

    let mut major = None;
    let mut minor = None;
    let mut patch = None;
    let mut generator_name = None;

    for line in cache_content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') || trimmed.starts_with("//") {
            continue;
        }
        if let Some(pos) = trimmed.find(':') {
            let key = &trimmed[..pos];

            if key == "CMAKE_CACHE_MAJOR_VERSION"
                || key == "CMAKE_CACHE_MINOR_VERSION"
                || key == "CMAKE_CACHE_PATCH_VERSION"
            {
                if let Some(eq_pos) = trimmed.find('=') {
                    let val = trimmed[eq_pos + 1..].trim().to_string();
                    match key {
                        "CMAKE_CACHE_MAJOR_VERSION" => major = Some(val),
                        "CMAKE_CACHE_MINOR_VERSION" => minor = Some(val),
                        "CMAKE_CACHE_PATCH_VERSION" => patch = Some(val),
                        _ => {}
                    }
                }
            } else if key == "CMAKE_GENERATOR"
                && let Some(eq_pos) = trimmed.find('=')
            {
                generator_name = Some(trimmed[eq_pos + 1..].trim().to_string());
            }
        }
    }

    let build_system = toolchain.build_system.get_or_insert_default();
    build_system.name = "CMake".to_string();
    build_system.version = match (major, minor, patch) {
        (Some(maj), Some(min), Some(pat)) => format!("{}.{}.{}", maj, min, pat),
        _ => {
            log::warn!("Could not find complete CMake version variables in CMakeCache.txt");
            String::new()
        }
    };

    let generator = toolchain.generator.get_or_insert_default();
    if let Some(name) = generator_name {
        generator.name = name.clone();
        if name.contains("Ninja") {
            let exec_name = "ninja";
            generator.version = extract_version(exec_name);
        } else {
            log::info!("Unknown generator: {}. Cannot extract version", name);
        }
    }

    log::trace!(
        "Found the following partial toolchain configuration: {:?}",
        toolchain
    );
    toolchain
}

fn read_compiler_cmake_file(cmake_files_dir: &Path) -> Option<String> {
    let compiler_file_path = cmake_files_dir.join("CMakeCXXCompiler.cmake");
    match fs::read_to_string(&compiler_file_path) {
        Ok(content) => Some(content),
        Err(_) => {
            log::warn!(
                "Could not open '{}' for parsing toolchain information",
                compiler_file_path.display()
            );
            None
        }
    }
}

fn parse_compiler_cmake_file(compiler_file_content: &str, toolchain: &mut Toolchain) {
    log::trace!("Parsing CMakeCXXCompiler.cmake content");
    for line in compiler_file_content.lines() {
        let trimmed = line.trim();
        if !trimmed.to_lowercase().starts_with("set(") || !trimmed.ends_with(')') {
            continue;
        }
        // Extract everything inside the set( ... )
        let inner = trimmed[4..trimmed.len() - 1].trim();
        if inner.is_empty() {
            continue;
        }
        let (key, val): (&str, &str) = if inner.contains('"') {
            // Quoted value; e.g. set(CMAKE_CXX_COMPILER "path/to/compiler")
            let parts: Vec<&str> = inner.split('"').collect();
            if parts.len() >= 2 {
                (parts[0].trim(), parts[1])
            } else {
                continue;
            }
        } else {
            // Unquoted value; e.g. set(CMAKE_CXX_COMPILER_LINKER_VERSION 1.0)
            if let Some(space_idx) = inner.find(char::is_whitespace) {
                (inner[..space_idx].trim(), inner[space_idx..].trim())
            } else {
                continue;
            }
        };

        if key == "CMAKE_CXX_COMPILER_ID" {
            log::trace!("Found CMAKE_CXX_COMPILER_ID = {}", val);
            toolchain.cxx_compiler.get_or_insert_default().name = val.to_string();
        } else if key == "CMAKE_CXX_COMPILER_VERSION" {
            log::trace!("Found CMAKE_CXX_COMPILER_VERSION = {}", val);
            toolchain.cxx_compiler.get_or_insert_default().version = val.to_string();
        } else if key == "CMAKE_C_COMPILER_ID" {
            log::trace!("Found CMAKE_C_COMPILER_ID = {}", val);
            toolchain.c_compiler.get_or_insert_default().name = val.to_string();
        } else if key == "CMAKE_C_COMPILER_VERSION" {
            log::trace!("Found CMAKE_C_COMPILER_VERSION = {}", val);
            toolchain.c_compiler.get_or_insert_default().version = val.to_string();
        } else if key == "CMAKE_CXX_COMPILER_LINKER_ID" {
            log::trace!("Found CMAKE_CXX_COMPILER_LINKER_ID = {}", val);
            toolchain.linker.get_or_insert_default().name = val.to_string();
        } else if key == "CMAKE_CXX_COMPILER_LINKER_VERSION" {
            log::trace!("Found CMAKE_CXX_COMPILER_LINKER_VERSION = {}", val);
            toolchain.linker.get_or_insert_default().version = val.to_string();
        }
    }
    log::info!("{:#?}", toolchain);
}

fn git_commit_from_output(success: bool, git_command_out: Vec<u8>) -> Option<String> {
    if success {
        let hash_str = String::from_utf8(git_command_out).ok()?;
        return Some(hash_str.trim().to_string());
    }
    None
}

fn extract_git_commit(build_dir: &str) -> Option<String> {
    let output = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(build_dir)
        .output()
        .ok()?;
    git_commit_from_output(output.status.success(), output.stdout)
}

/// Extracts the build information from the build directory.
pub fn extract_build_info(build_dir: &str) -> BuildInfo {
    let toolchain = match get_cmake_cache_content(build_dir) {
        Ok(cache_content) => {
            log::trace!("Found CMakeCache.txt file. Assuming CMake build");
            let mut toolchain = parse_cmake_cache(&cache_content);
            match get_cmake_files_dir(build_dir, &toolchain) {
                Some(cmake_files_dir) => {
                    if let Some(compiler_content) = read_compiler_cmake_file(&cmake_files_dir) {
                        parse_compiler_cmake_file(&compiler_content, &mut toolchain);
                    }
                }
                None => {
                    log::warn!("Could not find CMakeFiles directory in build directory");
                }
            }
            Some(toolchain)
        }
        Err(_) => {
            log::info!("Could not find CMakeFiles in build directory. Likely not a CMake build");
            None
        }
    };
    let git_commit = extract_git_commit(build_dir);
    BuildInfo {
        toolchain,
        git_commit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMAKE_CACHE_WITH_FULL_VERSION: &str = "\
# This is a comment
// CMake cache
CMAKE_CACHE_MAJOR_VERSION:STATIC=3
CMAKE_CACHE_MINOR_VERSION:STATIC=28
CMAKE_CACHE_PATCH_VERSION:STATIC=1
CMAKE_GENERATOR:INTERNAL=Ninja
CMAKE_CXX_COMPILER:FILEPATH=/usr/bin/clang++
";

    const CMAKE_CACHE_WITHOUT_VERSION: &str = "\
CMAKE_GENERATOR:INTERNAL=Unix Makefiles
SOME_OTHER_KEY:STRING=value
";

    const COMPILER_FILE: &str = "\
# compiler file comment
set(CMAKE_CXX_COMPILER_ID \"Clang\")
set(CMAKE_CXX_COMPILER_VERSION 22.1.0)
set(CMAKE_C_COMPILER_ID \"GNU\")
set(CMAKE_C_COMPILER_VERSION 21.1.8)
set(CMAKE_CXX_COMPILER_LINKER_ID \"GNU\")
set(CMAKE_CXX_COMPILER_LINKER_VERSION 2.42)
set(CMAKE_CXX_STANDARD 20)
";

    #[test]
    fn parse_cmake_version_and_generator() {
        let toolchain = parse_cmake_cache(CMAKE_CACHE_WITH_FULL_VERSION);
        assert_eq!(toolchain.build_system.as_ref().unwrap().name, "CMake");
        assert_eq!(toolchain.build_system.as_ref().unwrap().version, "3.28.1");
        assert_eq!(toolchain.generator.as_ref().unwrap().name, "Ninja");
    }

    #[test]
    fn missing_version_variable_yields_empty_version() {
        let toolchain = parse_cmake_cache(CMAKE_CACHE_WITHOUT_VERSION);
        assert_eq!(
            toolchain.build_system.as_ref().unwrap().version,
            String::new()
        );
        assert_eq!(toolchain.generator.as_ref().unwrap().name, "Unix Makefiles");
    }

    #[test]
    fn parse_compiler_cmake_file_extracts_toolchain() {
        let mut toolchain = Toolchain::default();
        parse_compiler_cmake_file(COMPILER_FILE, &mut toolchain);

        let cxx = toolchain.cxx_compiler.as_ref().unwrap();
        assert_eq!(cxx.name, "Clang");
        assert_eq!(cxx.version, "22.1.0");

        let c = toolchain.c_compiler.as_ref().unwrap();
        assert_eq!(c.name, "GNU");
        assert_eq!(c.version, "21.1.8");

        let linker = toolchain.linker.as_ref().unwrap();
        assert_eq!(linker.name, "GNU");
        assert_eq!(linker.version, "2.42");
    }

    #[test]
    fn parse_compiler_cmake_file_ignores_irrelevant_content() {
        let mut toolchain = Toolchain::default();
        parse_compiler_cmake_file(
            "# only a comment\nset(CMAKE_CXX_STANDARD 20)\n",
            &mut toolchain,
        );
        assert!(toolchain.cxx_compiler.is_none());
    }

    #[test]
    fn select_cmake_files_dir_prefers_matching_version() {
        let dirs = vec![
            PathBuf::from("CMakeFiles/3.28.1"),
            PathBuf::from("CMakeFiles/3.28.0"),
        ];
        let toolchain = Toolchain {
            build_system: Some(Tool {
                name: "CMake".to_string(),
                version: "3.28.0".to_string(),
            }),
            ..Default::default()
        };
        let found = select_cmake_files_dir(&dirs, &toolchain).unwrap();
        assert_eq!(found.file_name().unwrap().to_str().unwrap(), "3.28.0");
    }

    #[test]
    fn select_cmake_files_dir_falls_back_to_newest_when_version_unknown() {
        let dirs = vec![
            PathBuf::from("CMakeFiles/3.26.0"),
            PathBuf::from("CMakeFiles/3.29.3"),
        ];
        let toolchain = Toolchain::default();
        let found = select_cmake_files_dir(&dirs, &toolchain).unwrap();
        assert_eq!(found.file_name().unwrap().to_str().unwrap(), "3.29.3");
    }

    #[test]
    fn select_cmake_files_dir_returns_none_when_no_matching_version() {
        let dirs = vec![PathBuf::from("CMakeFiles/3.26.0")];
        let toolchain = Toolchain {
            build_system: Some(Tool {
                name: "CMake".to_string(),
                version: "3.30.0".to_string(),
            }),
            ..Default::default()
        };
        assert!(select_cmake_files_dir(&dirs, &toolchain).is_none());
    }

    #[test]
    fn git_commit_from_output_parses_successful_output() {
        assert_eq!(
            git_commit_from_output(true, b"abc123def\n".to_vec()),
            Some("abc123def".to_string())
        );
    }

    #[test]
    fn git_commit_from_output_ignores_failed_status() {
        assert_eq!(git_commit_from_output(false, b"abc123def\n".to_vec()), None);
    }

    #[test]
    fn git_commit_from_output_ignores_invalid_utf8() {
        assert_eq!(git_commit_from_output(true, vec![0xff, 0xfe]), None);
    }
}
