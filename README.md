# BL706 PAC and Embassy bring-up

Initial Bouffalo Lab BL702/704/706 support, targeting **Pinecil V2 / BL706**.
Hardware bring-up is in progress; startup, clocks and peripheral drivers still need
board validation. This is a research foundation; it does not yet implement a complete
soldering iron controller.

The workspace contains:

- `bl706-pac`: the official SVD register blocks, generated with **chiptool** and
  reproducible corrections. Only the documented subset has been audited.
- `bl706-hal`: RAM startup, configurable RC/XTAL/DLL clocks, GPIO, UART0 TX, CLIC,
  Embassy executor/time driver, interrupt-driven async I²C/ADC, PWM, TIMER0/1,
  watchdog, reset and a RAM-only blocking/async NOR flash driver.
- `examples/pinecil-v2`: peripheral probes, a sampling cycle with no heater output,
  clock/watchdog diagnostics, a read-only flash probe, FPU and isolated atomic probes.
  All examples log over RTT channel 0 (4 KiB, nonblocking); no UART wiring is needed.

## Build

The default example builds a bootable flash image and uses the patched probe-rs
runner with RTT. See [flash workflow](docs/flashing.md) for the debugger and backup
requirements. From the workspace root:

```sh
cargo run -r
```

The OLED example uses the async `ssd1306` driver and `embedded-graphics`:

```sh
cargo run -r --bin display
```

It draws `Embassy BL706` and an animated marker on the 96×16 screen while a
separate Embassy task logs a one-second RTT heartbeat. See [display](docs/display.md)
for the board configuration and hardware validation.

For tasks that must preempt CPU work in thread mode, the HAL also provides a
[software interrupt executor](docs/executor.md). Its HIL example checks timer
and I2C progress, a shared async mutex and FP context during a busy thread.

The explicit `firmware` aliases below retain the original RAM-only debug builds.

```sh
nix develop
cargo firmware --locked
cargo firmware-fpu --locked
```

`flake.lock`, `Cargo.lock`, `rust-toolchain.toml` and the chiptool commit are pinned.
Nix provides Rust, `rust-src`, chiptool, uv, LLVM and OpenOCD. The first Cargo build
fetches dependencies; subsequent builds can use `--offline`. The IMC examples also
have an isolated Nix build with vendored Cargo dependencies:

```sh
nix build .#firmware
# result/bin/{heartbeat,display,fusb302,i2c_latency,interrupt_executor,gpio_irq,analog,sampling,system,flash_probe}.elf
nix flake check
```

The Nix checks build IMC firmware and regenerate the PAC for a byte comparison.
`tools/check.sh` additionally checks IMFC/build-std and the atomic diagnostic binary.

| Configuration | Target | ABI | IRQ FP context |
|---|---|---|---|
| IMC | `riscv32imc-unknown-none-elf` | ilp32 | No FP instructions |
| IMFC, `fpu` | `riscv32imfc-unknown-none-elf` | ilp32f | f0..f31, fcsr, mstatus |

IMFC is a built-in Rust tier-3 target. The `firmware-fpu` alias rebuilds `core` with
`-Zbuild-std=core`; no custom JSON target is needed. **Both configurations exclude A.**
The HAL rejects A targets and IMFC builds without the `fpu` feature.

ELF files are under `target/<target>/release/`. The `firmware` aliases produce
**RAM ELFs for JTAG/GDB**, without a boot header. The default flash build has
physical flash load addresses; the Cargo runner adds the boot header checksums
and retains RAM symbols in `heartbeat.flash.elf`. A plain objcopy output is not
a substitute for this preparation.

```sh
# Isolated binary containing three intentional A instructions:
cargo build --locked --release -p pinecil-v2-bringup --bin atomics \
  --no-default-features --features atomic-probe --target riscv32imc-unknown-none-elf

# PAC, both configurations, clippy, host tests, ELF layout and instruction encodings:
bash tools/check.sh
```

Diagnostics go to `/home/okhsunrog/tmp_zfs/codex_tmp/bl706/checks/`;
set `BL706_DIAGNOSTICS` on other machines. Normal builds use `target/`.

## PAC

```sh
bash tools/generate-pac.sh
uv run --no-project tools/check_svd.py
```

The official SVD and both andelf versions are preserved unchanged in `svd/vendor/`.
`patch_svd.py` applies corrections before chiptool; `finish_pac.py` removes Cortex-M
glue and normalizes the generator banner. Do not edit generated `lib.rs` manually.

The PAC does not enforce peripheral ownership. Its safe MMIO accessors can violate
HAL invariants; do not mix direct PAC writes with an active HAL driver.

## Scope and limitations

- One hart, M-mode, a spinning thread executor and one software interrupt executor.
  No nested interrupts or sleep support; interrupt task polls must be short.
- Init-time RC32M, XTAL32M and DLL96/144M profiles; frequencies are nominal, not measured.
  No live clock switching, application XIP execution, RF, USB or DMA support.
  Flash boot uses a small XIP stub followed by execution entirely from RAM.
- Watchdog reset mode and explicit system reset are implemented; see [system](docs/system.md).
- NOR flash implements blocking and async `embedded-storage` traits with a reserved
  region and explicit device-ID check. It requires a RAM-only execution and board/SPI
  contract; the actual flash chip is unknown. The default probe only reads.
  No storage format, wear leveling or XIP restoration; see [flash](docs/flash.md).
- UART0 driver: TX on GPIO22; no RX support. Examples use RTT instead.
- GPIO inputs/outputs and all five interrupt-driven async `Wait` modes, with cancellation.
  FUSB302 INT_N is GPIO16; no debounce or deep-sleep wakeup. See [GPIO](docs/gpio.md).
- ADC: finite single-ended scans of 1..12 entries on GPIO18/19/20 and GND,
  raw/right-aligned samples and optional gain trim from a ready eFuse shadow.
  No differential/continuous/DMA support.
- PWM: five BCLK channels, normal polarity, `SetDutyCycle`, one-shot period IRQ.
  TIMER0/1: raw compare/preload, async wait or a static ISR driver; `mtime` remains
  independent. See [ADC/PWM/TIMER](docs/analog.md).
- `analog` and `sampling` leave PWM unrouted and hold GPIO21 low. No heater regulator
  or qualified heating protection is implemented.
- I²C: 1..256-byte packets; `write_read` with a 1..4-byte sub-address uses hardware
  repeated START. Other transaction shapes and empty packets return `Unsupported`
  before bus activity. No automatic bit-bang bus recovery.
- The ISR services the two-word I²C FIFO through an internal 256-byte buffer; tasks
  wait for completion. No caller buffer pointers are retained by the ISR. See [I²C](docs/i2c.md).
- FUSB302 probes do not implement USB-PD policy or request a higher voltage.
- The time queue supports eight distinct waiting wakers, an initial configuration limit.

See [sources](docs/sources.md), [PAC audit](docs/pac-audit.md),
[atomics and FPU](docs/atomics.md), [hardware assumptions and checklist](docs/hardware.md)
and [local validation](docs/validation.md).
