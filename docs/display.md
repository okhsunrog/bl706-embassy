# Pinecil V2 OLED

From the workspace root, using the patched probe-rs and existing Pico JTAG wiring:

```sh
cargo run -r --bin display
```

The example replaces the active flash application, with the same backup and
[flash boot contract](flashing.md) as `heartbeat`. It needs no UART or button
presses. The heater GPIO21 remains an output low.

## Driver and board settings

- `ssd1306` 0.10 with its async feature implements the display protocol and framebuffer.
- `embedded-graphics` 0.8 draws `Embassy BL706` with `FONT_6X10` and a moving 3×3 marker.
- The HAL implements `embedded-hal-async` I²C; there is no example-local I²C protocol driver.
- SSD1306-compatible 96×16 OLED, 7-bit address `0x3c`, GPIO10/11 I²C and
  active-low reset on GPIO3. These board parameters were checked against IronOS.
- `DisplayRotation::Rotate0` uses A1/C8 and columns 0..95, corresponding to
  IronOS's flipped layout for this panel.
- The crate resets the panel with `embassy_time::Delay`, then the example waits
  100 ms before initialization. Precharge phase2 is 15 and contrast 26, matching
  IronOS's long precharge and default contrast.

One Embassy task draws a frame, awaits its I²C flush, then awaits 250 ms. A second
task holds the heater low and writes an RTT heartbeat every second. Initialization
or bus errors are logged and stop only the display task; heartbeat keeps running.
This is a bring-up example, without an iron UI, temperature control or USB-PD policy.

## Hardware validation, 2026-11-04

The first RAM-loaded example initialized the OLED and repeatedly flushed frames
while the independent heartbeat continued. The initial two-line, 5×8 rendering
was reported as distorted; the final 6×10 rendering with board precharge/contrast
was confirmed readable with a moving marker by the user on the actual Pinecil.

The final example was also programmed and verified through `cargo run -r --bin display`,
booted from flash and produced both tasks' RTT logs. Full-frame I²C flushes took
about 21.75 ms at the HAL's nominal 100 kHz setting. This is MCU timer evidence,
not an external bus-frequency measurement. The flash payload was 34,784 bytes.

Three MCP reset/monitor cycles each reached `OLED frame=8`. Switching flash from
display to heartbeat, back to display, and programming display again also passed:
fresh RTT heartbeat in all three runs, with OLED frames in both display runs.
The two display programming/verification/start checks took about 12.6 seconds
through frame4. The complete offline suite and x86_64-linux `nix flake check`
passed with the display included in IMC/IMFC builds, clippy and ELF audits.

This validates the executor, timer wakeups, OLED reset/output and interrupt-driven
async I²C writes with this panel. It does not validate all HAL peripherals, I²C
error recovery, repeated START reads, worst-case IRQ latency or oscillator accuracy.
The larger example also exposed and verified the fix for the SPI/cache wrap
configuration described in [flashing.md](flashing.md).
