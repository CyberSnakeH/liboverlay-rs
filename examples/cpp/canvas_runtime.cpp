#include "liboverlay_canvas.hpp"
#include <cmath>
#include <iostream>
#include <stdexcept>
#include <thread>
static void require(bool ok, const char *message) {
  if (!ok)
    throw std::runtime_error(message);
}
int main() {
  try {
    auto options = overlay_canvas_default_options();
    options.width = 400;
    options.height = 300;
    options.flags = OVERLAY_ALLOW_SOFTWARE_GPU;
    OverlayCanvas *canvas = nullptr;
    require(overlay_canvas_create(&options, &canvas) == OVERLAY_OK,
            "canvas create");
    struct Cleanup {
      OverlayCanvas *value;
      ~Cleanup() { overlay_canvas_destroy(value); }
    } cleanup{canvas};
    OverlayCanvasInfo info{};
    require(overlay_canvas_info(canvas, &info) == OVERLAY_OK &&
                info.width == 400 && info.height == 300,
            "canvas geometry");
    int32_t wrong = 0;
    std::thread worker([&] { wrong = overlay_canvas_destroy(canvas); });
    worker.join();
    require(wrong == OVERLAY_WRONG_THREAD, "wrong-thread ownership");
    OverlayUiEvent events[OVERLAY_MAX_EVENTS]{};
    size_t count = 999;
    OverlayCommand bad{};
    bad.kind = OVERLAY_CMD_RECT;
    bad.width = NAN;
    OverlayFrame frame{OVERLAY_CANVAS_ABI_VERSION, 0, &bad, 1};
    require(overlay_canvas_submit(canvas, &frame, events, OVERLAY_MAX_EVENTS,
                                  &count) == OVERLAY_INVALID_ARGUMENT &&
                count == 0,
            "invalid frame was accepted");
    require(overlay_canvas_info(canvas, &info) == OVERLAY_OK &&
                info.input_mode == OVERLAY_INPUT_PASSIVE,
            "invalid frame changed handle");
    require(overlay_canvas_submit(canvas, &frame, events, 1, &count) ==
                OVERLAY_INVALID_ARGUMENT,
            "event capacity bound");
    require(overlay_canvas_set_input(canvas, OVERLAY_INPUT_POINTER) ==
                OVERLAY_OK,
            "pointer input");
    if (info.backend == OVERLAY_BACKEND_X11)
      require(overlay_canvas_set_input(canvas, OVERLAY_INPUT_MODAL) ==
                  OVERLAY_UNSUPPORTED,
              "X11 modal support overclaimed");
    require(overlay_canvas_set_input(canvas, OVERLAY_INPUT_PASSIVE) ==
                OVERLAY_OK,
            "input release");
    liboverlay::Frame drawing;
    drawing.rect({20, 20, 60, 40}, 0xffad52ff, 0, true);
    drawing.text({20, 80}, "generic ABI");
    auto descriptor = drawing.descriptor();
    require(overlay_canvas_submit(canvas, &descriptor, events,
                                  OVERLAY_MAX_EVENTS, &count) >= 0 &&
                count == 0,
            "valid frame submission");
    std::cout << "generic ABI: ownership, validation, input modes and "
                 "rendering passed\n";
  } catch (const std::exception &e) {
    std::cerr << e.what() << '\n';
    return 1;
  }
}
