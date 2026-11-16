cmake_minimum_required(VERSION 3.20)
project(liboverlay LANGUAGES C CXX)
find_program(LIBOVERLAY_CARGO cargo REQUIRED)
find_package(Threads REQUIRED)
set(LIBOVERLAY_ROOT "${CMAKE_CURRENT_SOURCE_DIR}")
set(LIBOVERLAY_TARGET_DIR "${CMAKE_CURRENT_BINARY_DIR}/cargo-target")
set(LIBOVERLAY_LIBRARY "${LIBOVERLAY_TARGET_DIR}/release/liboverlay_c.so")
file(GLOB_RECURSE LIBOVERLAY_RUST_SOURCES CONFIGURE_DEPENDS
    "${LIBOVERLAY_ROOT}/src/*.rs" "${LIBOVERLAY_ROOT}/ffi/src/*.rs")
add_custom_command(OUTPUT "${LIBOVERLAY_LIBRARY}"
    COMMAND "${LIBOVERLAY_CARGO}" build --locked --release -p liboverlay-c
        --manifest-path "${LIBOVERLAY_ROOT}/Cargo.toml" --target-dir "${LIBOVERLAY_TARGET_DIR}"
    DEPENDS ${LIBOVERLAY_RUST_SOURCES} "${LIBOVERLAY_ROOT}/Cargo.toml"
        "${LIBOVERLAY_ROOT}/ffi/Cargo.toml" "${LIBOVERLAY_ROOT}/Cargo.lock"
    WORKING_DIRECTORY "${LIBOVERLAY_ROOT}" VERBATIM)
add_custom_target(liboverlay_build DEPENDS "${LIBOVERLAY_LIBRARY}")
add_library(liboverlay_c SHARED IMPORTED GLOBAL)
set_target_properties(liboverlay_c PROPERTIES IMPORTED_LOCATION "${LIBOVERLAY_LIBRARY}"
    IMPORTED_NO_SONAME TRUE INTERFACE_INCLUDE_DIRECTORIES "${LIBOVERLAY_ROOT}/include"
    INTERFACE_LINK_LIBRARIES Threads::Threads)
add_dependencies(liboverlay_c liboverlay_build)
add_library(liboverlay::overlay ALIAS liboverlay_c)
