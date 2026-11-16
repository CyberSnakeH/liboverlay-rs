#ifndef LIBOVERLAY_CANVAS_H
#define LIBOVERLAY_CANVAS_H
#include "liboverlay.h"

#ifdef __cplusplus
extern "C" {
#endif

/* Additive API: the original dashboard ABI remains supported. */
#define OVERLAY_CANVAS_ABI_VERSION 1u
#define OVERLAY_UNSUPPORTED (-5)
#define OVERLAY_CONFLICT (-6)
#define OVERLAY_MAX_COMMANDS 8192u
#define OVERLAY_MAX_WIDGETS 128u
#define OVERLAY_MAX_EVENTS 256u
#define OVERLAY_MAX_SHORTCUTS 16u
#define OVERLAY_INPUT_PASSIVE 0u
#define OVERLAY_INPUT_POINTER 1u
#define OVERLAY_INPUT_MODAL 2u
#define OVERLAY_TARGET_UNKNOWN 0u
#define OVERLAY_TARGET_WAYLAND 1u
#define OVERLAY_TARGET_X11 2u /* May be Xorg or XWayland. */
#define OVERLAY_TARGET_XWAYLAND 3u
#define OVERLAY_SUPPORT_UNKNOWN 0u
#define OVERLAY_SUPPORT_AVAILABLE 1u
#define OVERLAY_SUPPORT_UNAVAILABLE 2u
#define OVERLAY_EVENT_CLICK 1u
#define OVERLAY_EVENT_CHANGE 2u
#define OVERLAY_EVENT_SHORTCUT 3u
#define OVERLAY_EVENT_SHORTCUT_LOST 4u
#define OVERLAY_CMD_RECT 1u
#define OVERLAY_CMD_LINE 2u
#define OVERLAY_CMD_CIRCLE 3u
#define OVERLAY_CMD_TRIANGLE 4u
#define OVERLAY_CMD_TEXT 5u
#define OVERLAY_CMD_CLIP 6u
#define OVERLAY_CMD_RESET_CLIP 7u
#define OVERLAY_CMD_PANEL_BEGIN 10u
#define OVERLAY_CMD_PANEL_END 11u
#define OVERLAY_CMD_LABEL 12u
#define OVERLAY_CMD_BUTTON 13u
#define OVERLAY_CMD_CHECKBOX 14u
#define OVERLAY_CMD_SLIDER 15u
#define OVERLAY_CMD_SEPARATOR 16u
#define OVERLAY_FILLED 1u
#define OVERLAY_TEXT_CENTER_X 2u
#define OVERLAY_TEXT_RIGHT 4u
#define OVERLAY_TEXT_CENTER_Y 8u
#define OVERLAY_TEXT_BOTTOM 16u
/* Hyprland modifier masks, independent of the application's keyboard layout. */
#define OVERLAY_MOD_SHIFT 1u
#define OVERLAY_MOD_CTRL 4u
#define OVERLAY_MOD_ALT 8u
#define OVERLAY_MOD_SUPER 64u

typedef struct OverlayCanvas OverlayCanvas;
typedef struct OverlayString {
  const char *data;
  size_t length;
} OverlayString;
typedef struct OverlayCanvasOptions {
  uint32_t abi_version, struct_size;
  uint32_t width, height;  /* Both zero: fill the selected output. */
  uint32_t backend, flags; /* Only OVERLAY_ALLOW_SOFTWARE_GPU is accepted. */
  uint32_t input_mode, target_pid; /* 0: no automatic target tracking. */
  OverlayString title, output;     /* Empty output selects automatically. */
} OverlayCanvasOptions;

typedef struct OverlayTarget {
  uint32_t visible, protocol;
  float x, y, width, height; /* Fractions of the output's logical extent. */
  char output[128];
} OverlayTarget;

typedef struct OverlayCanvasInfo {
  uint32_t backend, width, height, input_mode;
  uint32_t keyboard_capture, global_shortcuts;
  OverlayTarget target; /* No target: visible=1, viewport=(0,0,1,1). */
  char output[128];
} OverlayCanvasInfo;

/* Geometry is in logical surface pixels. Colors are unmultiplied 0xRRGGBBAA.
 * RECT/CLIP/PANEL: x,y,width,height. CIRCLE: x,y=center, width=radius.
 * LINE: x,y to x2,y2. TRIANGLE: x,y / x2,y2 / x3,y3.
 * TEXT: x,y anchor; height=font size. RECT/CIRCLE/TRIANGLE: FILLED or stroke.
 * TEXT alignment flags may combine one horizontal and one vertical alignment.
 * PANEL_BEGIN needs a stable nonzero id and width/height > 0; panels cannot
 * nest. LABEL/BUTTON/CHECKBOX/SLIDER/SEPARATOR appear sequentially inside a
 * panel. Interactive widgets need distinct nonzero ids, also distinct from
 * panel ids. CHECKBOX value=0/1; SLIDER value in [minimum,maximum], minimum <
 * maximum. Text is bounded UTF-8, without embedded NUL; no trailing NUL is
 * required. A frame allows <=128 widgets, <=8192 commands and <=1 MiB of total
 * text. */
typedef struct OverlayCommand {
  uint32_t kind, id, color, flags;
  float x, y, width, height, stroke, value, minimum, maximum;
  float x2, y2, x3, y3;
  OverlayString text;
} OverlayCommand;
typedef struct OverlayFrame {
  uint32_t abi_version, reserved;
  const OverlayCommand *commands;
  size_t count;
} OverlayFrame;
typedef struct OverlayUiEvent {
  uint32_t kind, id;
  float value;
  uint32_t reserved;
} OverlayUiEvent;

OverlayCanvasOptions overlay_canvas_default_options(void);
/* Handle ownership/thread rules and last-error reporting match liboverlay.h.
 * All input strings and command data are copied; no foreign pointers retained.
 * Options require exact struct_size and ABI version. *out=NULL on failure. */
int32_t overlay_canvas_create(const OverlayCanvasOptions *options,
                              OverlayCanvas **out);
int32_t overlay_canvas_destroy(OverlayCanvas *canvas);
/* Refreshes cached target geometry, follows output changes, then reports
 * surface dimensions. Call before constructing pixel coordinates for the next
 * frame. */
int32_t overlay_canvas_info(OverlayCanvas *canvas, OverlayCanvasInfo *out);
int32_t overlay_canvas_set_input(OverlayCanvas *canvas, uint32_t mode);
int32_t overlay_canvas_follow(OverlayCanvas *canvas, uint32_t pid);
/* Validates the whole frame before input/rendering. events must have capacity
 * >=OVERLAY_MAX_EVENTS; count is always initialized when non-NULL. Apply events
 * even on BUSY or a render error; they are delivered once, not replayed. */
int32_t overlay_canvas_submit(OverlayCanvas *canvas, const OverlayFrame *frame,
                              OverlayUiEvent *events, size_t capacity,
                              size_t *count);
/* One key name (e.g. Insert/F8) plus modifier mask. Currently Hyprland Lua API;
 * other desktops return UNSUPPORTED. Existing bindings return CONFLICT.
 * No helper process or application control socket is used. SHORTCUT events
 * arrive in submit(), even without focus. Caller decides whether to act on
 * them. SHORTCUT_LOST reports event-stream disconnect; registration must be
 * renewed. Runtime bindings are removed on normal destruction; config reload
 * may remove bindings independently. Supported registration does not imply
 * active hardware. */
int32_t overlay_canvas_shortcut(OverlayCanvas *canvas, OverlayString key,
                                uint32_t modifiers, uint32_t id);
int32_t overlay_canvas_unbind(OverlayCanvas *canvas, uint32_t id);
/* Standalone query for an external window. Hyprland supports native/XWayland;
 * X11 fallback uses _NET_WM_PID and RandR. No matching window: OK, visible=0.
 * No available integration: UNSUPPORTED. Output filter is optional. */
int32_t overlay_target_query(uint32_t pid, OverlayString output,
                             OverlayTarget *out);
int32_t overlay_active_pid(uint32_t *out);

#ifdef __cplusplus
}
#endif
#endif
