# Pinecil V2 assumptions and hardware checklist

## First-run contract

This section describes `cargo firmware` RAM-only builds. The default flash build
has a separate [boot and flashing contract](flashing.md).

- BL706, one hart, machine mode, cold reset/ROM ISP; do not attach over running IronOS.
- Debugger loads all ELF PT_LOAD segments at VMA=LMA. Startup does not copy `.data`
  from flash because the debugger loads it; startup clears `.bss`.
- `.init` and clock transitions use ITCM `0x22010000..0x22013fff`; remaining sections
  and stack use the lower 48 KiB OCRAM `0x22020000..0x2202bfff`. Upper OCRAM 16 KiB
  is reserved for possible reset/ROM EM allocation.
- ITCM/OCRAM are accessible after reset without changing cache partition. Verify
  this for the actual BL706/ROM and debug-reset sequence.
- RC32M is available after reset. Default init selects RC32M with CPU/BCLK divisors1;
  trim is not calibrated. Only `system` attempts XTAL/DLL144 and explicitly falls
  back on XTAL timeout. See [clock assumptions](system.md).
- Bus-divider latch code runs in ITCM without stack accesses: SDK warns that
  latching through `0x40000ffc` briefly stops BCLK/OCRAM.
- CLIC uses SDK layout: IP+0x000, IE+0x400, CFG+0x800, global+0xc00.
  `mtvec=handler|3`, `mtvt=vector table`, both bases aligned to 64 bytes.
  No nesting: handlers do not enable MIE.
- Board I²C pull-ups exist; actual rates/rise times have not been measured.
- Flash probe additionally requires the RAM/SPI/pad/timing contract in [flash.md](flash.md).

## Board mapping

Compared with U15 in the production [v2.0 schematic](https://files.pine64.org/doc/Pinecil/Pinecil_schematic_v2.0_20220608.pdf)
and SDK alternate functions. Schematic UART1 labels name board nets; the UART
driver can route controller **UART0 TX** to GPIO22. Examples now use RTT instead.

| Signal | GPIO | Example behavior |
|---|---|---|
| Tip_Control | 21 | GPIO output low; no heater drive |
| UART TX | 22 | Optional driver: function7, signal6 → UART0_TXD; examples leave it alone |
| UART RX | 23 | No RX driver |
| I²C SCL | 10 | function6 |
| I²C SDA | 11 | function6 |
| OLED RESET_N | 3 | `display` drives an active-low reset; other examples leave it alone |
| FUSB302 INT_N | 16 | Pull-up input, GPIO IRQ/level waits |
| Button A / B | 25 / 28 | Pull-down inputs |

IronOS `Pins.h` reverses the SCL/SDA labels. Its `pinmux_config.h` selects function6
on both pins, so that discrepancy does not alter hardware routing; it matters for
future GPIO bus recovery. GPIO23..28 honor `GPIO_USE_PSRAM__IO`: SDK can remap pad
configuration to indices32..37. Other board pins are not configured automatically.
The SF pad supplies IE/pulls/drive, while FUNC_SEL remains in the original
GPIO23..28 register. Both must be configured. Input gates for GPIO9..13 in
HBN_IRQ_MODE are also configured explicitly rather than inherited from ROM.

The default I2C rate remains nominally 100 kHz. Applications can call
`i2c.set_frequency(clocks, 300_000)` to use Pinecil V2 IronOS's requested rate;
equal phases are rounded down in frequency and SCL synchronization adds delay.
Transactions enable the same one-cycle deglitch filter as IronOS. Neither the
register calculation nor a successful transfer measures the actual SCL rate.

## Examples

- `heartbeat`: RTT banner, seconds/time, buttons and PD_IRQ level.
- `display`: async `ssd1306` / `embedded-graphics` text and animation at address
  0x3c, with a separate RTT heartbeat; see [display](display.md).
- `fusb302`: once per second, `write_read(0x22, [DEVICE_ID=0x01], 1 byte)` and Result.
  No read-to-clear INTERRUPT access, RESET writes or USB-PD negotiation.
- `i2c_latency`: reads nine FUSB302 registers0x01..0x09, blocking the executor for
  20 ms inside the first Pending poll while IRQs remain enabled. The ISR must
  advance a transfer larger than FIFO. Compiled only.
- `fpu`: IMFC f32 result, FS/fcsr and Timer.
- `gpio_irq`: independent GPIO waiters and RTT reporter: button A any-edge with
  timeout/cancel/rearm, button B rising/falling, FUSB302 INT_N low/high. No debounce
  or clearing of FUSB302 interrupt registers; see [gpio.md](gpio.md).
- `atomics`: isolated one-shot probes on owned RAM; see [atomics.md](atomics.md).
- `analog`: ADC CH10/9/8 scan, TIMER1 compare, unrouted PWM0 period IRQ, trim/sample logs.
- `sampling`: TIMER0 ISR owns ADC and unrouted PWM1, with three compare phases and
  latest-frame Signal. GPIO21 stays low; see [analog.md](analog.md).
- `system`: XTAL/DLL144 attempt/fallback, clock/reset logs and watchdog feed every100 ms.
  It does not call system reset, but stopping feed/debug halt may cause watchdog reset.
- `flash_probe`: JEDEC/status and a 16-byte read only. Compile-only trait wrappers
  are retained but never called. It creates no runtime writable region.

FUSB302 probes do not require one fixed ID because FUSB302/FUSB302B variants differ.
Record the actual U1 marking and ID.

## Hardware validation order

For first bring-up, remove the tip, use current-limited power and check Tip_Control
low. Obtain a verified backup of stock flash and a ROM ISP recovery path. A RAM ELF
is not an instruction or authorization to erase flash. The first RAM heartbeat
has now run on BL706 with RTT; see [validation results](validation.md).

The Pico CMSIS-DAP wiring and OpenOCD target are in `tools/pinecil-v2.cfg`.
After cold booting into ROM ISP, start OpenOCD from the repository root:

```sh
openocd -f tools/pinecil-v2.cfg
gdb target/riscv32imc-unknown-none-elf/release/heartbeat -x tools/ram-load.gdb
```

The GDB script rejects a PC outside ROM, loads only RAM, compares all ELF
sections and stops at `__bl706_main` before HAL initialization. This sequence
has been executed on the first board. After inspecting the state, explicitly
resume the core; detaching alone can leave it halted:

```gdb
monitor resume
detach
quit
```

Stop OpenOCD before attaching an RTT reader through probe-rs/flashprobe-mcp;
both programs cannot own the USB probe at the same time. No UART is needed.

If ITCM/OCRAM cannot be loaded after reset, establish the debugger memory/reset
contract first rather than changing flash/cache registers speculatively.

1. **Reset/RAM:** record debugger ID, CSRs, ROM/SiFive revision. Check entry
   `0x22010000`, OCRAM stack, cleared bss and preserved data after both cold/debug reset.
2. **Clocks/UART:** step transitions; read HBN_GLB/dividers, measure bit time/8N1,
   UART timeout and bounded logging. Use an external reference, not mtime, to
   measure RC error. For `system`, verify XTAL cap codes/readiness/fallback, DLL96/144
   rates, safe divisor ordering, no OCRAM corruption, UART/I²C/PWM/TIMER rates and
   mtime remaining1 MHz across profiles. Check RC32K startup and measured32K/1K.
3. **Trap/CLIC:** trigger a controlled exception, inspect BL706_TRAP and mtvec/mtvt,
   enable MTIME, verify mret and absence of permanently pending IRQs.
4. **Time/executor:** run heartbeat for >=10 minutes, multiple deadlines, expired
   deadlines, earlier reprogramming, cancel/reuse. Put mtime near low32 rollover;
   verify hi/lo/hi reads and measure RC drift.
5. **GPIO:** check buttons/PD_IRQ polarity and GPIO25/28 SF-pad remap. OE changes
   must not pulse the heater. Test IRQ60, both detectors, rising/falling/any-edge,
   simultaneous pins, cancel/rearm without stale events. Active levels complete
   immediately; edges wait for a new event. Check CLR pulse and latched-low INT_N
   without IRQ storm or timer/log starvation. The read-only FUSB302 example does
   not clear INT_N. Use a separate free input to measure pulse width/latency under
   I²C/mtime load; never inject test signals onto heater control.
6. **I²C analyzer:** observe START,0x44(W),0x01,repeated START,0x45(R),byte,master NACK,
   STOP. Confirm GPIO10=SCL/GPIO11=SDA, approximately100 kHz, clock stretching and
   no overruns.
7. **I²C failures:** NACK at an unused address, held SDA/SCL, timeout, mid-transfer
   future cancellation, then a new successful transfer. Check CLIC/I²C masks off,
   m_en=0 and cleared FIFOs after Drop. Controller cleanup is not recovery from a
   physically stuck line. Caller RX must remain unchanged on error/cancel.
8. **FIFO/transactions:** external test slave, lengths1,2,3,4,5,7,8,9,255,256 and
   sub-address lengths1..4; check byte order/no padding. Unsupported/zero/257-byte
   shapes must not start bus activity. Inject data NACK, arbitration/FIFO errors
   where possible.
9. **Concurrency:** timer tasks continue during I²C/timeout. Test completion before
   waker registration, IRQ between polls, late END/final word and cancel cleanup.
   Run `i2c_latency`: require stalled=true, correct nine bytes/analyzer trace and
   completion despite20 ms without future polling. ISR must stop controller/IRQ
   itself. Measure latency under nonnested IRQs/critical sections, including RTT
   logging. RTT uses nonblocking writes, but formatting still consumes task time.
10. **FPU:** check FS/fcsr/result. Fill all FPRs with distinct sentinels under the
    debugger, trigger MTIME/I²C with handler FP clobbers, compare f0..f31/fcsr/mstatus
    after mret. Measure stack watermark.
11. **Atomics:** isolated binary; record LR/SC and AMO results/traps. One success
    does not justify an A target. Other aliases/DMA need separate tests; no arbitrary MMIO.
12. **TIMER:** check IRQ53 in `analog`, IRQ52 in `sampling`, independent mtime and
    preservation of adjacent channel/watchdog TCCR/TCDR/TCER/TCMR fields. Verify
    capture progression, actual compare/preload intervals including the SDK two-tick
    discrepancy, stop/restart, rollover, cancel/rearm and pending-before-waker.
    Measure low-frequency sources rather than assuming readiness.
13. **ADC:** start with GND via `Adc<()>`, then known voltages on CH8/9/10 measured
    externally. Verify tags (negative=23), eight-entry order,12/14/16-bit alignment,
    scans1..12, timeout/cancel/reuse and overrun/underrun/saturation. Record shadow
    readiness, trim enable/parity and raw/calibrated differences. Check100 us restart
    pause, settling and reference. Heating interference needs a separate explicitly
    prepared test; do not enable heating in these examples.
14. **PWM:** start unrouted with IRQ49. Use a separately verified fixture/pad for
    waveform, not GPIO21. Check divider/period,0/50/100%,enable/disable/Drop-low,
    STS_TOP timeout and clear1→0. Multiple channels/cancel must preserve neighbors.
15. **Dry-run sampling:** with GPIO21 low, verify three phases/frames. Delay tasks
    with MIE=1: ISR must continue and Signal may replace frames. Delay IRQs until
    compares coalesce or ADC errors; verify dry-run stop. Measure latency/stack
    under I²C/GPIO/mtime load. This does not qualify heater operation.
16. **Watchdog/reset:** after verifying the recovery path, measure expiry/feed/stop
    on both sources and dividers, then intentionally omit feed. Inspect independent
    HBN/WSR flags before/after explicit clear, preservation of TIMER0/1, behavior
    while debug-halted and Drop leaving WDT active. Separately call system reset
    and verify its ROM/flash boot path; a RAM ELF is not self-restarting.
17. **Flash read:** first establish the unsafe RAM/pad/timing/SPI contract, record
    chip marking/JEDEC ID, status/protection and capacity against its datasheet.
    Capture SPI commands/clock, compare boundary reads with an independent dump,
    and check controller timeouts/Drop without attempting XIP resumption.
18. **Flash write, separately authorized:** retain a verified original backup;
    confirm a reserved sector outside firmware/data and the full expected JEDEC ID,
    then obtain approval immediately before writing hardware. Test page-crossing
    program,4 KiB erase/readback, bounds/alignment/0→1/protection, blocking/async,
    IRQ progress, cancellation with WIP still active and next-request behavior.
    Verify timeout poisoning and unchanged neighboring sectors. Power-loss/endurance
    tests require separate expendable data and are not implied by compile success.

None of these items has been completed on hardware. Normal USB flashing additionally
needs boot headers/flash parameters, a boot2/ROM contract, XIP/cache setup, a linker
layout and a verified image-packaging path.
