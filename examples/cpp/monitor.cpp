#include "liboverlay.hpp"
#include <algorithm>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <thread>

// Application logic: independent of egui and Rust.
struct CpuMonitor {
    float average = 0.0f;
    float threshold = 80.0f;
    bool alerts_enabled = true;

    void update(float sample) {
        average = average * 0.9f + std::clamp(sample, 0.0f, 100.0f) * 0.1f;
    }
    bool alert_active() const { return alerts_enabled && average >= threshold; }
    OverlayState state() const {
        return {average, threshold, alerts_enabled ? 1u : 0u, alert_active() ? 1u : 0u};
    }
    void apply(const OverlayActions& actions) {
        if (actions.settings_changed) {
            threshold = actions.alert_threshold;
            alerts_enabled = actions.alerts_enabled != 0;
        }
    }
};

class CpuSampler {
    std::uint64_t previous_total_ = 0;
    std::uint64_t previous_idle_ = 0;
public:
    float sample() {
        std::ifstream file("/proc/stat");
        std::string line, label;
        if (!std::getline(file, line)) throw std::runtime_error("Cannot read /proc/stat");
        std::istringstream fields(line);
        fields >> label;
        std::uint64_t values[8]{};
        for (auto& value : values)
            if (!(fields >> value)) throw std::runtime_error("Invalid /proc/stat CPU counters");
        std::uint64_t total = 0;
        for (const auto value : values) total += value;
        const auto idle = values[3] + values[4]; // idle + iowait; exclude guest double counting
        float load = 0.0f;
        if (previous_total_ != 0 && total > previous_total_ && idle >= previous_idle_) {
            const double elapsed = static_cast<double>(total - previous_total_);
            load = static_cast<float>(100.0 * (1.0 - static_cast<double>(idle - previous_idle_) / elapsed));
        }
        previous_total_ = total;
        previous_idle_ = idle;
        return std::clamp(load, 0.0f, 100.0f);
    }
};

int main(int argc, char** argv) {
    try {
        auto config = overlay_default_config();
        double seconds = 30.0;
        bool simulate = false;
        for (int i = 1; i < argc; ++i) {
            const std::string arg = argv[i];
            if (arg == "--self-test") {
                CpuMonitor monitor;
                monitor.update(100.0f);
                if (std::abs(monitor.average - 10.0f) > 0.001f) return 1;
                monitor.apply({1, 5.0f, 1, 0});
                if (!monitor.alert_active()) return 1;
                monitor.apply({1, 5.0f, 0, 0});
                if (monitor.alert_active()) return 1;
                std::cout << "C++ business logic: passed\n";
                return 0;
            }
            if (arg == "--allow-software") config.flags |= OVERLAY_ALLOW_SOFTWARE_GPU;
            else if (arg == "--simulate") simulate = true;
            else if (arg == "--seconds" && i + 1 < argc) seconds = std::stod(argv[++i]);
            else if (arg == "--backend" && i + 1 < argc) {
                const std::string backend = argv[++i];
                if (backend == "wayland") config.backend = OVERLAY_BACKEND_WAYLAND;
                else if (backend == "x11") config.backend = OVERLAY_BACKEND_X11;
                else if (backend != "auto") throw std::runtime_error("Unknown backend");
            } else throw std::runtime_error("Usage: overlay_monitor [--seconds N] [--backend auto|wayland|x11] [--simulate] [--allow-software] [--self-test]");
        }
        if (!std::isfinite(seconds) || seconds <= 0.0 || seconds > 86400.0)
            throw std::runtime_error("Seconds must be in 0..86400");
        liboverlay::Session overlay(config);
        CpuMonitor monitor;
        CpuSampler sampler;
        const auto start = std::chrono::steady_clock::now();
        auto next_sample = start;
        unsigned ticks = 0;
        while (std::chrono::duration<double>(std::chrono::steady_clock::now() - start).count() < seconds) {
            const auto now = std::chrono::steady_clock::now();
            if (now >= next_sample) {
                const double time = std::chrono::duration<double>(now - start).count();
                monitor.update(simulate ? static_cast<float>(50.0 + 40.0 * std::sin(time)) : sampler.sample());
                next_sample = now + std::chrono::milliseconds(200);
            }
            OverlayActions actions{};
            const auto status = overlay.tick(monitor.state(), actions);
            if (ticks == 0) std::cout << "overlay ready" << std::endl;
            monitor.apply(actions);
            if (actions.settings_changed)
                std::cout << "settings: threshold=" << monitor.threshold << " alerts=" << monitor.alerts_enabled << std::endl;
            ++ticks;
            if (status == OVERLAY_CLOSED) {
                std::cout << "close requested\n";
                break;
            }
            std::this_thread::sleep_for(std::chrono::milliseconds(16));
        }
        std::cout << "ticks=" << ticks << " cpu=" << monitor.average << " clean shutdown\n";
    } catch (const std::exception& error) {
        std::cerr << error.what() << '\n';
        return 1;
    }
}
