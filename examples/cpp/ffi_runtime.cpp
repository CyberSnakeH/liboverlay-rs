#include "liboverlay.h"
#include <chrono>
#include <cmath>
#include <cstdint>
#include <iostream>
#include <limits>
#include <stdexcept>
#include <thread>

static void require(bool condition, const char* message) {
    if (!condition) throw std::runtime_error(message);
}

int main() {
    Overlay* handle = nullptr;
    try {
        auto config = overlay_default_config();
        config.backend = OVERLAY_BACKEND_WAYLAND;
        config.flags = OVERLAY_ALLOW_SOFTWARE_GPU;
        require(overlay_create(&config, &handle) == OVERLAY_OK, "creation failed");
        OverlayState state{42.0f, 80.0f, 1, 0};
        OverlayActions actions{};
        int32_t foreign_tick = 0, foreign_destroy = 0;
        std::thread foreign([&] {
            foreign_tick = overlay_tick(handle, &state, &actions);
            foreign_destroy = overlay_destroy(handle);
        });
        foreign.join();
        require(foreign_tick == OVERLAY_WRONG_THREAD, "foreign tick accepted");
        require(foreign_destroy == OVERLAY_WRONG_THREAD, "foreign destroy consumed handle");
        state.cpu_percent = std::numeric_limits<float>::quiet_NaN();
        require(overlay_tick(handle, &state, &actions) == OVERLAY_INVALID_ARGUMENT, "NaN accepted");
        state.cpu_percent = 42.0f;
        for (unsigned i = 0; i != 30; ++i) {
            require(overlay_tick(handle, &state, &actions) == OVERLAY_OK, "rendering failed after rejected calls");
            std::this_thread::sleep_for(std::chrono::milliseconds(16));
        }
        const auto destroyed = overlay_destroy(handle);
        handle = nullptr;
        require(destroyed == OVERLAY_OK, "destruction failed");
        std::cout << "C++ live ABI: wrong-thread rejection, invalid state, rendering and cleanup passed\n";
    } catch (const std::exception& error) {
        char diagnostic[1024]{};
        overlay_last_error(diagnostic, sizeof diagnostic);
        std::cerr << error.what() << ": " << diagnostic << '\n';
        overlay_destroy(handle);
        return 1;
    }
}
