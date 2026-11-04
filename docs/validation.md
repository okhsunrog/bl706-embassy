# Local validation

2026-11-04, x86_64-linux/NixOS. These are the initial offline results;
peripheral validation on hardware remains pending.

## Checks

| Check | Result |
|---|---|
| `nix build .#chiptool --no-link` | Pinned chiptool/SVD parser build with lockfile and fixed-output hashes |
| `nix flake check` | Packages/checks/devShell evaluation and both x86_64-linux sandbox checks |
| `nix develop -c bash tools/check.sh` | Full suite below, exit0 |
| SVD comparison | Immutable input hashes, semantic comparison, corrected bases/IRQ/gates/access assertions |
| `cargo fmt --all --check` | Pass |
| Host PAC build/test/doc-test | Builds; generated PAC has no unit tests |
| Host I²C engine | 10 tests: all TX/RX lengths1..256 without waiter polls, bounded service, END/error/late-word races, cancel/stale IRQ/reuse, caller-storage independence |
| Host GPIO engine | 7 tests: all32 mode fields, levels/edges, early events, level/unmask race, multiple pins, cancel/stale IRQ/rearm |
| Host ADC/PWM/TIMER | 10 tests: scans1..12, errors/tags/extra/cancel/reuse, restart gate, packing/resolution, all4096 trims/parity/sign/clamp, PWM/TIMER bounds and shared fields |
| Host clocks/watchdog | 4 tests: rate plans, mux encodings, bounds, timeout rounding |
| Host NOR engine | 8 tests: region/ID/bounds/alignment, command encoding, page splitting, program/erase/readback, preconditions/protection, persistent pending, timeout/poison |
| IMC release | heartbeat, fusb302, i2c_latency, gpio_irq, analog, sampling, system, flash_probe and isolated atomics link |
| IMFC release/build-std=core | Same eight normal examples plus fpu link |
| Clippy `-D warnings` | IMC with atomic-probe and IMFC with fpu pass; generated PAC permits standard generator lints |
| ELF32 layout | RAM bounds, VMA=LMA, entry,8 KiB stack,80-entry vector table/handlers7/25/48/49/52/53/60,64-byte mtvec/mtvt alignment, ABI |
| Clock/flash placement | Both clock-latch functions in ITCM; four compile-only flash wrappers retained in RAM |
| Instruction encodings | No A opcodes in17 normal ELF files; exactly3 labelled probes in atomics; no FP in IMC; all32 FPR spills/reloads in IMFC |

The complete `tools/check.sh` suite also passes after replacing example UART
output with nonblocking RTT (4 KiB up-channel 0). The HAL UART driver remains
available, but the examples no longer initialize GPIO22 for logging. Host checks
and ELF audits do not establish that RTT or peripheral initialization works on
the MCU. First-board logs are kept under
`/home/okhsunrog/tmp_zfs/codex_tmp/bl706/first-hardware/`.

The 39 host tests exercise production logic but not the physical MCU. PAC generation
from the git checkout and the Nix archive build yields the same SHA-256:

```text
dff5e9208db52a2122ad5a50a5c6d1cde5d74a1cdd70020484ab0db28c10190f
```

Only the optional generator git-commit/date banner is normalized. Normal firmware
built in the checkout and sandbox may differ in debug paths; cross-path ELF bit
reproducibility was not tested.

Current full logs are outside the repository:

```text
/home/okhsunrog/tmp_zfs/codex_tmp/bl706/system-flash-check.log
/home/okhsunrog/tmp_zfs/codex_tmp/bl706/system-flash-nix-check.log
```

## Practical limits

The I²C model checks its ISR engine and completion stop/mask, not BL706/CLIC, W1C
side effects, END/NAK timing, IRQ latency or physical STOP. `i2c_latency` is compiled
but has never demonstrated nine-byte progress during20 ms executor blockage on a
board. See [I²C ownership/cancellation](i2c.md).

GPIO tests run the state engine, not target MMIO/waker/future Drop glue, pads,
CLR pulse timing, detector routing or IRQ return. `gpio_irq` is compiled only;
see [gpio.md](gpio.md).

ADC/PWM/TIMER tests cover logic/ADC engine, not physical FIFO/tags/W1C, callbacks,
accuracy, settling, capture, compare offsets, waveforms, IRQ latency or fail-off timing.
`analog`/`sampling` leave PWM unrouted and GPIO21 low, without a heater regulator.
See [analog.md](analog.md).

Clock plans/ITCM placement do not prove the MMIO transition sequence, oscillator
stability, calibration or reset behavior. Watchdog timing/expiry/reset flags/debug
behavior have not been measured. See [system.md](system.md).

NOR tests run the production engine against memory-backed transport, not SF_CTRL,
SRAM arbitration, SPI timing, real WIP, power loss/endurance or XIP/cache behavior.
The flash example only reads at runtime; write/erase wrappers are linked but never
called. The actual chip ID and unsafe RAM/SPI contract are unconfirmed.
See [flash.md](flash.md).

FP spills/reloads do not replace a runtime register-sentinel test. Nix sandbox
success is not proof of startup or clock-tree operation. LR/SC/AMO results/traps on
the actual BL706 revision remain unknown. All pending device checks and write
prerequisites are listed in [hardware.md](hardware.md).

The AArch64 Nix environment is declared but was not built here. No QEMU or BL706
register-level simulator was run; emulating a different RISC-V SoC would not validate BL706.
