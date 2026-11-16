#include "liboverlay_canvas.h"
#include <assert.h>
#include <string.h>
_Static_assert(sizeof(OverlayCanvasOptions) == 64, "options ABI");
_Static_assert(sizeof(OverlayCanvasInfo) == 304, "info ABI");
_Static_assert(sizeof(OverlayFrame) == 24, "frame ABI");
_Static_assert(sizeof(OverlayUiEvent) == 16, "event ABI");
_Static_assert(sizeof(OverlayCommand) == 80,
               "command ABI on supported Linux x86-64/aarch64");
_Static_assert(sizeof(OverlayTarget) == 152, "target ABI");
int main(void) {
  OverlayCanvasOptions options = overlay_canvas_default_options();
  assert(options.abi_version == OVERLAY_CANVAS_ABI_VERSION);
  assert(options.struct_size == sizeof options);
  OverlayCanvas *canvas = NULL;
  options.struct_size--;
  assert(overlay_canvas_create(&options, &canvas) == OVERLAY_INVALID_ARGUMENT);
  assert(canvas == NULL);
  assert(overlay_canvas_destroy(NULL) == OVERLAY_OK);
  size_t count = 123;
  assert(overlay_canvas_submit(NULL, NULL, NULL, 0, &count) ==
         OVERLAY_INVALID_ARGUMENT);
  assert(count == 0);
  OverlayTarget target;
  assert(overlay_target_query(0, (OverlayString){NULL, 0}, &target) ==
         OVERLAY_INVALID_ARGUMENT);
  return 0;
}
