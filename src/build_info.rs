use std::{
    fs::{self, File},
    io::{BufRead, BufReader},
    path::PathBuf,
    process::Command,
};

#[derive(Debug, Default)]
pub struct Tool {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Default)]
pub struct Toolchain {
    pub build_system: Option<Tool>, // e.g. CMake
    pub generator: Option<Tool>,    // e.g. Ninja
    pub cxx_compiler: Option<Tool>, // e.g. clang++
    pub c_compiler: Option<Tool>,   // e.g. clang
    pub linker: Option<Tool>,       // e.g. lld
}

#[derive(Debug, Default)]
pub struct BuildInfo {
    pub toolchain: Option<Toolchain>,
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

fn get_cmake_cache_file(build_dir: &str) -> Result<File, std::io::Error> {
    let cmake_cache_path = build_dir.to_string() + "/CMakeCache.txt";
    File::open(cmake_cache_path)
}

fn get_cmake_files_dir(build_dir: &str, toolchain: &Toolchain) -> Option<PathBuf> {
    log::trace!("Searching for CMakeFiles directory");
    let cmake_files_dir = build_dir.to_string() + "/CMakeFiles";
    let entries = fs::read_dir(cmake_files_dir).ok()?;

    let mut version_dirs: Vec<PathBuf> = entries
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
                &build_system.version
            );
        } else {
            log::trace!(
                "Could not find CMakeFiles directory corresponding to version {}",
                &build_system.version
            );
        }
        return found_dir;
    } else {
        log::warn!(
            "The CMake version is unknown. Searching for toolchain info in latest version's CMakeFiles"
        )
    };

    log::trace!("Using the latest CMake Files version");
    // Sort alphabetically / numerically so the latest version is at the end
    version_dirs.sort();
    let newest_version = version_dirs.pop();
    newest_version
}

fn parse_cmake_cache(cmake_cache_file: File) -> Toolchain {
    log::trace!("Parsing CMake cache");
    let mut toolchain = Toolchain::default();

    let mut major = None;
    let mut minor = None;
    let mut patch = None;
    let mut generator_name = None;

    let reader = BufReader::new(cmake_cache_file);
    for line in reader.lines().map_while(Result::ok) {
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
            } else if key == "CMAKE_GENERATOR" {
                if let Some(eq_pos) = trimmed.find('=') {
                    generator_name = Some(trimmed[eq_pos + 1..].trim().to_string());
                }
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

fn parse_compiler_cmake_file(cmake_files_dir: PathBuf, toolchain: &mut Toolchain) {
    log::trace!("Parsing CMakeFiles directory");
    let compiler_file_path = cmake_files_dir.join("CMakeCXXCompiler.cmake");
    let file = match File::open(&compiler_file_path) {
        Ok(f) => f,
        Err(_) => {
            log::warn!(
                "Could not open '{}' for parsing toolchain information",
                compiler_file_path.display()
            );
            return;
        }
    };

    let reader = BufReader::new(file);
    for line in reader.lines().map_while(Result::ok) {
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

        if key.contains("CMAKE_CXX_COMPILER_ID") {
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

fn extract_git_commit(build_dir: &str) -> Option<String> {
    let output = Command::new("git")
        .args(&["rev-parse", "HEAD"])
        .current_dir(build_dir)
        .output()
        .ok()?;
    if output.status.success() {
        let hash_str = String::from_utf8(output.stdout).ok()?;
        return Some(hash_str.trim().to_string());
    }
    None
}

pub fn extract_build_info(build_dir: &str) -> BuildInfo {
    let toolchain = match get_cmake_cache_file(&build_dir) {
        Ok(file) => {
            log::trace!("Found CMakeCache.txt file. Assuming CMake build");
            let mut toolchain = parse_cmake_cache(file);
            match get_cmake_files_dir(&build_dir, &toolchain) {
                Some(cmake_files_dir) => {
                    parse_compiler_cmake_file(cmake_files_dir, &mut toolchain);
                }
                None => {
                    log::warn!("Could not find CMakeFiles directory in build directory");
                }
            };
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
