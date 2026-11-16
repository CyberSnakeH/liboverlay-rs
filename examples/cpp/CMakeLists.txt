cmake_minimum_required(VERSION 3.20)
project(liboverlay_cpp_example LANGUAGES C CXX)

find_program(CARGO_EXECUTABLE cargo REQUIRED)
find_package(Threads REQUIRED)
get_filename_component(OVERLAY_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../.." ABSOLUTE)
set(OVERLAY_CARGO_TARGET "${CMAKE_CURRENT_BINARY_DIR}/cargo-target")
set(OVERLAY_LIBRARY "${OVERLAY_CARGO_TARGET}/release/liboverlay_c.so")
file(GLOB_RECURSE OVERLAY_RUST_SOURCES CONFIGURE_DEPENDS
    "${OVERLAY_ROOT}/src/*.rs" "${OVERLAY_ROOT}/ffi/src/*.rs")
add_custom_command(OUTPUT "${OVERLAY_LIBRARY}"
    COMMAND "${CARGO_EXECUTABLE}" build --locked --release -p liboverlay-c
            --manifest-path "${OVERLAY_ROOT}/Cargo.toml" --target-dir "${OVERLAY_CARGO_TARGET}"
    DEPENDS ${OVERLAY_RUST_SOURCES} "${OVERLAY_ROOT}/Cargo.toml"
            "${OVERLAY_ROOT}/ffi/Cargo.toml" "${OVERLAY_ROOT}/Cargo.lock"
    WORKING_DIRECTORY "${OVERLAY_ROOT}"
    VERBATIM)
add_custom_target(overlay_rust DEPENDS "${OVERLAY_LIBRARY}")
add_library(overlay_c SHARED IMPORTED)
set_target_properties(overlay_c PROPERTIES
    IMPORTED_LOCATION "${OVERLAY_LIBRARY}" IMPORTED_NO_SONAME TRUE)
add_dependencies(overlay_c overlay_rust)

add_executable(overlay_monitor monitor.cpp)
target_compile_features(overlay_monitor PRIVATE cxx_std_17)
target_compile_options(overlay_monitor PRIVATE -Wall -Wextra -Wpedantic -Werror)
target_include_directories(overlay_monitor PRIVATE "${OVERLAY_ROOT}/include")
target_link_libraries(overlay_monitor PRIVATE overlay_c Threads::Threads)

add_executable(overlay_abi_smoke abi_smoke.c)
target_compile_features(overlay_abi_smoke PRIVATE c_std_11)
target_compile_options(overlay_abi_smoke PRIVATE -Wall -Wextra -Wpedantic -Werror -UNDEBUG)
target_include_directories(overlay_abi_smoke PRIVATE "${OVERLAY_ROOT}/include")
target_link_libraries(overlay_abi_smoke PRIVATE overlay_c)

enable_testing()
add_executable(overlay_ffi_runtime ffi_runtime.cpp)
target_compile_features(overlay_ffi_runtime PRIVATE cxx_std_17)
target_compile_options(overlay_ffi_runtime PRIVATE -Wall -Wextra -Wpedantic -Werror)
target_include_directories(overlay_ffi_runtime PRIVATE "${OVERLAY_ROOT}/include")
target_link_libraries(overlay_ffi_runtime PRIVATE overlay_c Threads::Threads)
# Graphical fixture: invoked only by scripts/check_cpp.py on its private server.
add_test(NAME c_abi COMMAND overlay_abi_smoke)
add_test(NAME cpp_business_logic COMMAND overlay_monitor --self-test)

add_executable(overlay_canvas canvas.cpp)
target_compile_features(overlay_canvas PRIVATE cxx_std_17)
target_compile_options(overlay_canvas PRIVATE -Wall -Wextra -Wpedantic -Werror)
target_include_directories(overlay_canvas PRIVATE "${OVERLAY_ROOT}/include")
target_link_libraries(overlay_canvas PRIVATE overlay_c Threads::Threads)
add_executable(overlay_canvas_abi canvas_abi.c)
target_compile_features(overlay_canvas_abi PRIVATE c_std_11)
target_compile_options(overlay_canvas_abi PRIVATE -Wall -Wextra -Wpedantic -Werror -UNDEBUG)
target_include_directories(overlay_canvas_abi PRIVATE "${OVERLAY_ROOT}/include")
target_link_libraries(overlay_canvas_abi PRIVATE overlay_c)
add_test(NAME generic_c_abi COMMAND overlay_canvas_abi)
add_executable(overlay_canvas_runtime canvas_runtime.cpp)
target_compile_features(overlay_canvas_runtime PRIVATE cxx_std_17)
target_compile_options(overlay_canvas_runtime PRIVATE -Wall -Wextra -Wpedantic -Werror)
target_include_directories(overlay_canvas_runtime PRIVATE "${OVERLAY_ROOT}/include")
target_link_libraries(overlay_canvas_runtime PRIVATE overlay_c Threads::Threads)
# Invoked by scripts/check_canvas.py on an isolated compositor.
