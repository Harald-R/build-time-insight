cmake_minimum_required(VERSION 3.16)
project(build_time_insight_fixture CXX)

set(CMAKE_CXX_STANDARD 17)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
set(CMAKE_CXX_EXTENSIONS OFF)

# Needed to detect header usage details during analysis, which re-runs clang
# per unit using the recorded compile commands.
set(CMAKE_EXPORT_COMPILE_COMMANDS ON)

# Set to ON to compile every translation unit with `-ftime-trace` so the
# build directory gains one <tu>.json trace file per translation unit. Those
# files are the input to the analyzer.
option(ENABLE_FTIME_TRACE "Emit -ftime-trace per translation unit" OFF)

add_library(geometry_lib STATIC
    src/geometry.cpp
    src/shapes.cpp
)
target_include_directories(geometry_lib PUBLIC include)

add_executable(shapely src/main.cpp)
target_link_libraries(shapely PRIVATE geometry_lib)

add_library(edge_cases STATIC
    src/edge/edge_unused.cpp
    src/edge/edge_fwd.cpp
    src/edge/edge_needs.cpp
    src/edge/edge_diamond.cpp
    src/edge/edge_diamond2.cpp
)
target_include_directories(edge_cases PUBLIC include)

if(ENABLE_FTIME_TRACE)
    target_compile_options(geometry_lib PRIVATE -ftime-trace)
    target_compile_options(shapely PRIVATE -ftime-trace)
    target_compile_options(edge_cases PRIVATE -ftime-trace)
    message(STATUS "Build-time tracing enabled: -ftime-trace on every TU")
endif()