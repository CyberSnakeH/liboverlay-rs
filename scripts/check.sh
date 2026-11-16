#!/bin/sh
# Display-independent checks. Opt-in graphical suites are documented separately.
set -eu
cargo fmt --all --check
python3 -m py_compile scripts/*.py
cargo test --locked
cargo test --locked --workspace --all-features --all-targets
cargo test --locked --workspace --all-features --doc
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
RUSTDOCFLAGS='-D warnings' cargo doc --locked --workspace --all-features --no-deps
cargo build --locked --example diagnose
python3 scripts/check_failure_paths.py
cargo package --locked -p liboverlay-rs --allow-dirty --all-features
