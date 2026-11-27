cmake_minimum_required(VERSION 3.28)
project(build_time_insight_modules CXX)

set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# Needed for --header-usage and source-to-target mapping.
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# Set to ON to compile every translation unit with `-ftime-trace`.
option(ENABLE_FTIME_TRACE "Emit -ftime-trace per translation unit" OFF)

# Module library: primary interface, two partitions, and an implementation unit.
add_library(core)
target_sources(core
    PUBLIC FILE_SET CXX_MODULES FILES
        src/core.cppm
        src/core-shapes.cppm
        src/core-detail.cppm
    PRIVATE
        src/core.cpp
)
target_include_directories(core PUBLIC include)

# An executable that imports the module and also uses ordinary headers.
add_executable(modules_app
    src/main.cpp
    src/consumer.cpp
)
target_link_libraries(modules_app PRIVATE core)
target_include_directories(modules_app PRIVATE include)

set_target_properties(core modules_app PROPERTIES CXX_SCAN_FOR_MODULES ON)

if(ENABLE_FTIME_TRACE)
    target_compile_options(core PRIVATE -ftime-trace)
    target_compile_options(modules_app PRIVATE -ftime-trace)
    message(STATUS "Build-time tracing enabled: -ftime-trace on every TU")
endif()
