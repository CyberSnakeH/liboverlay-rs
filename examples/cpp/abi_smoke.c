#include "liboverlay.h"
#include <assert.h>
#include <math.h>
#include <string.h>

_Static_assert(sizeof(OverlayConfig) == 20, "config ABI");
_Static_assert(sizeof(OverlayState) == 16, "state ABI");
_Static_assert(sizeof(OverlayActions) == 16, "actions ABI");

int main(void) {
    assert(overlay_abi_version() == OVERLAY_ABI_VERSION);
    OverlayConfig config = overlay_default_config();
    Overlay* overlay = NULL;
    config.abi_version++;
    assert(overlay_create(&config, &overlay) == OVERLAY_INVALID_ARGUMENT);
    assert(overlay == NULL);
    char message[256];
    assert(overlay_last_error(message, sizeof message) > 1);
    assert(strstr(message, "ABI") != NULL);
    assert(overlay_tick(NULL, NULL, NULL) == OVERLAY_INVALID_ARGUMENT);
    assert(overlay_destroy(NULL) == OVERLAY_OK);
    return 0;
}
