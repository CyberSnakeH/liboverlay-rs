#include "liboverlay_canvas.hpp"
#include <chrono>
#include <cmath>
#include <iostream>
#include <thread>
int main(int argc, char **argv) {
  try {
    auto options = overlay_canvas_default_options();
    options.width = 500;
    options.height = 320;
    options.input_mode = OVERLAY_INPUT_POINTER;
    options.title = liboverlay::view("liboverlay generic canvas");
    double seconds = 10;
    for (int i = 1; i < argc; ++i) {
      const std::string arg = argv[i];
      if (arg == "--allow-software")
        options.flags |= OVERLAY_ALLOW_SOFTWARE_GPU;
      else if (arg == "--wayland")
        options.backend = OVERLAY_BACKEND_WAYLAND;
      else if (arg == "--x11")
        options.backend = OVERLAY_BACKEND_X11;
      else if (arg == "--seconds" && i + 1 < argc)
        seconds = std::stod(argv[++i]);
      else
        throw std::runtime_error("invalid argument");
    }
    if (!std::isfinite(seconds) || seconds <= 0 || seconds > 120)
      throw std::runtime_error("invalid duration");
    liboverlay::Canvas canvas(options);
    liboverlay::Frame frame;
    bool enabled = true;
    float opacity = 75;
    const auto start = std::chrono::steady_clock::now();
    while (
        std::chrono::duration<double>(std::chrono::steady_clock::now() - start)
            .count() < seconds) {
      (void)canvas.info();
      frame.clear();
      frame.rect({330, 30, 120, 80}, 0x4ddadcff, 2);
      frame.circle({390, 160}, 28, 0xffad52ff, 2);
      frame.line({330, 210}, {450, 270}, 0xffffffff, 2);
      frame.text({330, 290}, "C++ canvas", 14);
      frame.begin_panel(1, {20, 20, 260, 240});
      frame.label("Interface defined in C++");
      frame.checkbox(2, "Show", enabled);
      frame.slider(3, "Opacity", opacity, 0, 100);
      frame.button(4, "Close");
      frame.end_panel();
      const auto result = canvas.submit(frame);
      bool close = false;
      for (const auto &e : result.events) {
        if (e.kind == OVERLAY_EVENT_CHANGE && e.id == 2) {
          enabled = e.value != 0;
          std::cout << "enabled=" << enabled << std::endl;
        }
        if (e.kind == OVERLAY_EVENT_CHANGE && e.id == 3) {
          opacity = e.value;
          std::cout << "opacity=" << opacity << std::endl;
        }
        if (e.kind == OVERLAY_EVENT_CLICK && e.id == 4)
          close = true;
      }
      result.check();
      if (result.status == OVERLAY_CLOSED || close)
        break;
      static bool ready = false;
      if (!ready) {
        std::cout << "canvas ready" << std::endl;
        ready = true;
      }
      std::this_thread::sleep_for(std::chrono::milliseconds(16));
    }
    std::cout << "canvas clean shutdown" << std::endl;
  } catch (const std::exception &e) {
    std::cerr << e.what() << '\n';
    return 1;
  }
}
