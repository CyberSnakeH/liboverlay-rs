#ifndef LIBOVERLAY_H
#define LIBOVERLAY_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define OVERLAY_ABI_VERSION 1u
#define OVERLAY_OK 0
#define OVERLAY_CLOSED 1
#define OVERLAY_BUSY 2
#define OVERLAY_INVALID_ARGUMENT (-1)
#define OVERLAY_ERROR (-2)
#define OVERLAY_PANIC (-3)
#define OVERLAY_WRONG_THREAD (-4)
#define OVERLAY_BACKEND_AUTO 0u
#define OVERLAY_BACKEND_WAYLAND 1u
#define OVERLAY_BACKEND_X11 2u
#define OVERLAY_CLICK_THROUGH 1u
#define OVERLAY_ALLOW_SOFTWARE_GPU 2u

typedef struct Overlay Overlay;

typedef struct OverlayConfig {
    uint32_t abi_version;
    uint32_t width;  /* logical pixels */
    uint32_t height;
    uint32_t backend;
    uint32_t flags;  /* bitwise OR of OVERLAY_* flags */
} OverlayConfig;

/* CPU dashboard example schema. Custom Rust interfaces can define other schemas.
 * Percentages must be finite and in 0..100; boolean integers must be 0 or 1. */
typedef struct OverlayState {
    float cpu_percent;
    float alert_threshold;
    uint32_t alerts_enabled;
    uint32_t alert_active;
} OverlayState;

typedef struct OverlayActions {
    uint32_t settings_changed;
    float alert_threshold;
    uint32_t alerts_enabled;
    uint32_t close_requested;
} OverlayActions;

uint32_t overlay_abi_version(void);
OverlayConfig overlay_default_config(void);

/* All pointer arguments must be valid and aligned; distinct arguments must not
 * overlap. No pointers are retained after a call. All operations on a handle,
 * including destruction, must be serialized on the thread that created it.
 * Passing a dangling/foreign handle or concurrently accessing it is invalid.
 * Creation sets *out to NULL on failure. GPU rendering and transparency are
 * required; backend availability still depends on the desktop compositor. */
int32_t overlay_create(const OverlayConfig* config, Overlay** out);

/* Builds the Rust dashboard, processes input and queues rendering. Success is
 * not confirmation of scanout. Apply actions even on BUSY; they are not replayed.
 * Call regularly and pace the application loop. On PANIC, only destroy the handle.
 * The caller keeps authority over configuration and sends the updated state next tick. */
int32_t overlay_tick(Overlay* overlay, const OverlayState* state, OverlayActions* actions);

/* NULL is accepted. On the creator thread consumes the handle even on failure.
 * WRONG_THREAD leaves ownership with the caller. Never destroy a handle twice. */
int32_t overlay_destroy(Overlay* overlay);

/* Thread-local diagnostic from the last status-returning call. Returns required
 * bytes INCLUDING NUL. capacity=0 permits NULL. Otherwise capacity bytes must be
 * writable. Copies at most capacity-1 bytes and terminates; UTF-8 may be truncated.
 * Query before another API call clears the diagnostic. */
size_t overlay_last_error(char* buffer, size_t capacity);

#ifdef __cplusplus
}
#endif
#endif
