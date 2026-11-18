#include <liboverlay_canvas.hpp>
#include <iostream>
#include <poll.h>

static void draw_scene(liboverlay::Frame &frame, float s) {
  frame.rect({0, 0, 240 * s, 144 * s}, 0x000000ff, 0, true);
  frame.line({8 * s, 12 * s}, {48 * s, 12 * s}, 0xff0000ff, 4 * s);
  frame.circle({72 * s, 20 * s}, 12 * s, 0x00ff00ff, 0, true);
  frame.triangle({96 * s, 32 * s}, {112 * s, 8 * s}, {128 * s, 32 * s},
                 0x0000ffff);
  frame.text({144 * s, 8 * s}, "Canvas", 20 * s);
  frame.clip({0, 0, 4 * s, 4 * s});
  frame.clip({16 * s, 56 * s, 32 * s, 24 * s});
  frame.rect({4 * s, 44 * s, 56 * s, 48 * s}, 0xffff00ff, 0, true);
  frame.reset_clip();
  frame.rect({80 * s, 56 * s, 48 * s, 40 * s}, 0xff000080, 0, true);
  frame.rect({104 * s, 68 * s, 48 * s, 40 * s}, 0x0000ff80, 0, true);
}

int main(int argc, char **argv) {
  auto options = overlay_canvas_default_options();
  if (options.abi_version != OVERLAY_CANVAS_ABI_VERSION ||
      options.struct_size != sizeof(options))
    return 1;
  if (argc == 1)
    return 0;
  if (argc != 3)
    return 1;
  const std::string_view backend(argv[1]), scale(argv[2]);
  if ((backend != "wayland" && backend != "x11") ||
      (scale != "1" && scale != "1.25"))
    return 1;
  const bool wayland = backend == "wayland";
  options.backend = wayland ? OVERLAY_BACKEND_WAYLAND : OVERLAY_BACKEND_X11;
  options.flags = OVERLAY_ALLOW_SOFTWARE_GPU;
  if (wayland)
    options.output = liboverlay::view(scale == "1" ? "HEADLESS-1" : "HEADLESS-2");
  try {
    liboverlay::Canvas canvas(options);
    liboverlay::Frame frame;
    // Wayland negotiates logical coordinates. X11 takes application pixels.
    draw_scene(frame, !wayland && scale == "1.25" ? 1.25f : 1.0f);
    pollfd input{0, POLLIN, 0};
    for (;;) {
      const auto result = canvas.submit(frame);
      result.check();
      if (result.status != OVERLAY_OK && result.status != OVERLAY_BUSY)
        return 1;
      const int ready = poll(&input, 1, 16);
      if (ready != 0)
        return ready < 0 ? 1 : 0;
    }
  } catch (const std::exception &error) {
    std::cerr << error.what() << '\n';
    return 1;
  }
}
