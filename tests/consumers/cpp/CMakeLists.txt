cmake_minimum_required(VERSION 3.20)
project(overlay_consumer LANGUAGES CXX)

add_subdirectory("../source" liboverlay)
add_executable(overlay_consumer main.cpp)
target_compile_features(overlay_consumer PRIVATE cxx_std_17)
target_link_libraries(overlay_consumer PRIVATE liboverlay::overlay)
