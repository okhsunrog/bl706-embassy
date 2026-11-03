#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/.."
diagnostics=${BL706_DIAGNOSTICS:-/home/okhsunrog/tmp_zfs/codex_tmp/bl706/checks}
mkdir -p "$diagnostics/tmp"
export TMPDIR="$diagnostics/tmp"
before=$(sha256sum crates/bl706-pac/src/lib.rs)
bash tools/generate-pac.sh
after=$(sha256sum crates/bl706-pac/src/lib.rs)
test "$before" = "$after" || { echo 'Generated PAC was stale' >&2; exit 1; }
uv run --no-project tools/check_svd.py
cargo fmt --all --check
cargo test --locked -p bl706-pac
rustc --edition 2024 --test crates/bl706-hal/src/i2c_logic.rs -o "$diagnostics/i2c-logic-tests"
"$diagnostics/i2c-logic-tests"
rustc --edition 2024 --test crates/bl706-hal/src/gpio_irq.rs -o "$diagnostics/gpio-irq-tests"
"$diagnostics/gpio-irq-tests"
cargo firmware --locked
cargo firmware-fpu --locked
cargo build --locked --release -p pinecil-v2-bringup --bin atomics --features atomic-probe --target riscv32imc-unknown-none-elf
cargo clippy --locked --target riscv32imc-unknown-none-elf -p bl706-hal -p pinecil-v2-bringup --bins --features atomic-probe -- -D warnings
cargo clippy --locked --target riscv32imfc-unknown-none-elf -Zbuild-std=core -p bl706-hal -p pinecil-v2-bringup --bins --features fpu -- -D warnings
uv run --no-project tools/audit_elf.py
echo 'Offline source/build/encoding checks passed. No hardware was exercised.'
