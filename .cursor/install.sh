#!/usr/bin/env bash
# Idempotent Cloud Agent bootstrap for the dock workspace.
#
# The default image ships Rust 1.83, but several crates in this workspace
# (e.g. vendor/xai/workflow, cordis-render/markdown) require `edition = "2024"`,
# which is only stabilized in Rust >= 1.85. `reqwest` (default features) links
# against system OpenSSL, so pkg-config + libssl-dev must be present.
set -euo pipefail

# System libraries needed to build openssl-sys (via reqwest default features).
sudo apt-get update -qq
sudo apt-get install -y -qq --no-install-recommends pkg-config libssl-dev

# edition2024 support requires a modern stable toolchain.
rustup default stable
rustup component add rustfmt clippy >/dev/null 2>&1 || true

# Warm the build cache and validate the workspace compiles.
cargo build -p cordis-app
