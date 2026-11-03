#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
uv run --no-project tools/patch_svd.py
"${CHIPTOOL:-chiptool}" generate --svd svd/patched.svd --no-defmt --output crates/bl706-pac/src
uv run --no-project tools/finish_pac.py crates/bl706-pac/src/lib.rs
rustfmt --edition 2024 crates/bl706-pac/src/lib.rs
# chiptool's Cortex-M linker aliases are deliberately unused by our CLIC runtime.
rm crates/bl706-pac/src/device.x
