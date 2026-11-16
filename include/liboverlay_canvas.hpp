#ifndef LIBOVERLAY_CANVAS_HPP
#define LIBOVERLAY_CANVAS_HPP
#include "liboverlay.hpp"
#include "liboverlay_canvas.h"
#include <array>
#include <deque>
#include <string_view>

namespace liboverlay {
inline OverlayString view(std::string_view text) {
  return {text.data(), text.size()};
}
struct Point {
  float x, y;
};
struct Rect {
  float x, y, width, height;
};

// Owns every command and string until submission; reuse with clear().
class Frame final {
public:
  void clear() {
    commands_.clear();
    strings_.clear();
  }
  void rect(Rect r, uint32_t color, float stroke = 1, bool filled = false) {
    auto &c = add(OVERLAY_CMD_RECT);
    bounds(c, r);
    c.color = color;
    c.stroke = stroke;
    c.flags = filled ? OVERLAY_FILLED : 0;
  }
  void line(Point a, Point b, uint32_t color, float stroke = 1) {
    auto &c = add(OVERLAY_CMD_LINE);
    c.x = a.x;
    c.y = a.y;
    c.x2 = b.x;
    c.y2 = b.y;
    c.color = color;
    c.stroke = stroke;
  }
  void circle(Point center, float radius, uint32_t color, float stroke = 1,
              bool filled = false) {
    auto &c = add(OVERLAY_CMD_CIRCLE);
    c.x = center.x;
    c.y = center.y;
    c.width = radius;
    c.color = color;
    c.stroke = stroke;
    c.flags = filled ? OVERLAY_FILLED : 0;
  }
  void triangle(Point a, Point b, Point d, uint32_t color, bool filled = true,
                float stroke = 1) {
    auto &c = add(OVERLAY_CMD_TRIANGLE);
    c.x = a.x;
    c.y = a.y;
    c.x2 = b.x;
    c.y2 = b.y;
    c.x3 = d.x;
    c.y3 = d.y;
    c.color = color;
    c.stroke = stroke;
    c.flags = filled ? OVERLAY_FILLED : 0;
  }
  void text(Point p, std::string_view value, float size = 14,
            uint32_t color = 0xffffffff, uint32_t alignment = 0) {
    auto &c = add(OVERLAY_CMD_TEXT, value);
    c.x = p.x;
    c.y = p.y;
    c.height = size;
    c.color = color;
    c.flags = alignment;
  }
  void clip(Rect r) { bounds(add(OVERLAY_CMD_CLIP), r); }
  void reset_clip() { add(OVERLAY_CMD_RESET_CLIP); }
  void begin_panel(uint32_t id, Rect r) {
    auto &c = add(OVERLAY_CMD_PANEL_BEGIN);
    c.id = id;
    bounds(c, r);
  }
  void end_panel() { add(OVERLAY_CMD_PANEL_END); }
  void label(std::string_view value, uint32_t color = 0xffffffff) {
    add(OVERLAY_CMD_LABEL, value).color = color;
  }
  void button(uint32_t id, std::string_view label) {
    add(OVERLAY_CMD_BUTTON, label).id = id;
  }
  void checkbox(uint32_t id, std::string_view label, bool value) {
    auto &c = add(OVERLAY_CMD_CHECKBOX, label);
    c.id = id;
    c.value = value ? 1.0f : 0.0f;
  }
  void slider(uint32_t id, std::string_view label, float value, float minimum,
              float maximum) {
    auto &c = add(OVERLAY_CMD_SLIDER, label);
    c.id = id;
    c.value = value;
    c.minimum = minimum;
    c.maximum = maximum;
  }
  void separator() { add(OVERLAY_CMD_SEPARATOR); }
  OverlayFrame descriptor() const {
    return {OVERLAY_CANVAS_ABI_VERSION, 0, commands_.data(), commands_.size()};
  }

private:
  std::vector<OverlayCommand> commands_;
  std::deque<std::string> strings_;
  static void bounds(OverlayCommand &c, Rect r) {
    c.x = r.x;
    c.y = r.y;
    c.width = r.width;
    c.height = r.height;
  }
  OverlayCommand &add(uint32_t kind, std::string_view text = {}) {
    OverlayCommand c{};
    c.kind = kind;
    if (!text.empty()) {
      strings_.emplace_back(text);
      c.text = view(strings_.back());
    }
    commands_.push_back(c);
    return commands_.back();
  }

public:
  Frame() = default;
  // Command string pointers refer to owned deque elements. Do not copy them.
  Frame(const Frame &) = delete;
  Frame &operator=(const Frame &) = delete;
  Frame(Frame &&) = delete;
  Frame &operator=(Frame &&) = delete;
};
struct Submission {
  int32_t status = OVERLAY_OK;
  std::vector<OverlayUiEvent> events;
  std::string error;
  void check() const {
    if (status < 0)
      throw std::runtime_error(error);
  }
};
class Canvas final {
public:
  explicit Canvas(
      const OverlayCanvasOptions &options = overlay_canvas_default_options()) {
    check(overlay_canvas_create(&options, &handle_));
  }
  ~Canvas() noexcept { overlay_canvas_destroy(handle_); }
  Canvas(const Canvas &) = delete;
  Canvas &operator=(const Canvas &) = delete;
  Canvas(Canvas &&) = delete;
  Canvas &operator=(Canvas &&) = delete;
  OverlayCanvasInfo info() {
    OverlayCanvasInfo result{};
    check(overlay_canvas_info(handle_, &result));
    return result;
  }
  void input(uint32_t mode) { check(overlay_canvas_set_input(handle_, mode)); }
  void follow(uint32_t pid) { check(overlay_canvas_follow(handle_, pid)); }
  // Registration can be optional: callers may handle UNSUPPORTED/CONFLICT.
  int32_t shortcut(std::string_view key, uint32_t id, uint32_t modifiers = 0) {
    return overlay_canvas_shortcut(handle_, view(key), modifiers, id);
  }
  void unbind(uint32_t id) { check(overlay_canvas_unbind(handle_, id)); }
  Submission submit(const Frame &frame) {
    std::array<OverlayUiEvent, OVERLAY_MAX_EVENTS> events{};
    size_t count = 0;
    const auto descriptor = frame.descriptor();
    const auto status = overlay_canvas_submit(
        handle_, &descriptor, events.data(), events.size(), &count);
    // Events may have occurred before a renderer error: return them first.
    return {status,
            {events.begin(), events.begin() + count},
            status < 0 ? last_error() : std::string{}};
  }

private:
  ::OverlayCanvas *handle_ = nullptr;
  static void check(int32_t status) {
    if (status < 0)
      throw std::runtime_error(last_error());
  }
};
inline OverlayTarget target(uint32_t pid, std::string_view output = {}) {
  OverlayTarget result{};
  const auto status = overlay_target_query(pid, view(output), &result);
  if (status < 0 && status != OVERLAY_UNSUPPORTED)
    throw std::runtime_error(last_error());
  return result;
}
inline uint32_t active_pid() {
  uint32_t pid = 0;
  const auto status = overlay_active_pid(&pid);
  if (status < 0 && status != OVERLAY_UNSUPPORTED)
    throw std::runtime_error(last_error());
  return pid;
}
} // namespace liboverlay
#endif
