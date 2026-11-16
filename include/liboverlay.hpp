#ifndef LIBOVERLAY_HPP
#define LIBOVERLAY_HPP

#include "liboverlay.h"
#include <stdexcept>
#include <string>
#include <vector>

namespace liboverlay {
inline std::string last_error() {
    std::vector<char> text(overlay_last_error(nullptr, 0));
    overlay_last_error(text.data(), text.size());
    return std::string(text.data());
}

// Owns one handle on the constructing thread. No copy or cross-thread transfer.
class Session final {
public:
    explicit Session(const OverlayConfig& config = overlay_default_config()) {
        if (overlay_create(&config, &handle_) < 0)
            throw std::runtime_error(last_error());
    }
    ~Session() noexcept { overlay_destroy(handle_); }
    Session(const Session&) = delete;
    Session& operator=(const Session&) = delete;
    Session(Session&&) = delete;
    Session& operator=(Session&&) = delete;

    // Returns OK, BUSY or CLOSED. Throws on errors; actions is still initialized.
    int32_t tick(const OverlayState& state, OverlayActions& actions) {
        const auto status = overlay_tick(handle_, &state, &actions);
        if (status < 0) throw std::runtime_error(last_error());
        return status;
    }

private:
    ::Overlay* handle_ = nullptr;
};
}
#endif
