# Clocks, watchdog and reset

No profile has run on a board. Rates are calculated from selected sources and
dividers, not measured or confirmed stable. All examples remain RAM ELF files,
without a boot image or XIP startup.

## Clocks

`init()` retains the nominal RC32M default: CPU/BCLK/UART = 32 MHz.
`init_with_config(clock::Config)` freezes clocks before issuing peripheral tokens.
There is no public live clock switching, and `Clocks` has no public constructor or
fields. Repeated initialization returns `AlreadyInitialized` after success.
An invalid plan is rejected before MMIO. On XTAL timeout the CPU stays on RC32M,
no tokens are issued, and an explicit retry with `init()` is allowed.

```rust,ignore
let p = bl706_hal::init_with_config(
    bl706_hal::clock::Config::pinecil_v2_144mhz()
)?;
```

| Root | Reference | Typical CPU / BCLK, actual divisors | UART |
|---|---|---|---|
| RC32M | RC32M | 32 / 32 MHz, 1 / 1 | FCLK 32 MHz |
| XTAL32M | Crystal | 32 / 32 MHz, 1 / 1 | FCLK 32 MHz |
| DLL96M | RC32M or XTAL32M | 96 / 48 MHz, 1 / 2 | FCLK 96 MHz |
| DLL144M | RC32M or XTAL32M | 144 / 72 MHz, 1 / 2 | FCLK 144 MHz |

The implementation follows the SDK's **DLL** terminology and `GLB_Power_On_DLL`;
the RM uses PLL terminology for this clock path. It does not implement a separate
audio/RF PLL. Actual CPU/BCLK divisors 1..256 encode as N−1. Plans require exact
integer division, CPU <=160 MHz and BCLK <=80 MHz per SDK, and BCLK >=1 MHz in
integer MHz for the time driver. Arbitrary fractional rates are unsupported.

Initialization selects RC32M before configuring XTAL/DLL. Default XTAL cap codes
33/33 come from the Pinecil BSP and are configurable. The SDK settling delay follows
the cap-code write. XTAL_RDY is polled at most 120 times with finite nominal RC32M
delays. DLL power/reset/settling and output enables follow the SDK; no documented
DLL lock flag is available here. DLL96/144 enable IROM 2T access.

Both BCLK-latch functions run in ITCM with no stack/OCRAM access while BCLK stops.
An upward transition applies divisors on RC32M **before** selecting the fast root,
avoiding transient BCLK=144 MHz. The reset path selects the slow root before removing
dividers. ELF checks require `bl706_clock_to_rc32m` and `bl706_clock_apply` in ITCM;
waveforms and OCRAM correctness still require hardware validation.

`Clocks` supplies UART, PWM and TIMER rates. I²C derives four equal phases from BCLK,
rounding down to a nominal bus rate <=100 kHz. `mtime` divides BCLK to 1 MHz so
Embassy deadlines and peripheral timeouts do not speed up with the CPU.
ADC uses XCLK32M / 4 / 4 = 2 MHz; SF_CTRL uses XCLK / 4 = 8 MHz. UART selects FCLK,
not XCLK, and its divider is recalculated for DLL roots.

Init explicitly powers RC32K, selects it for F32K and waits the SDK startup delay
of more than 800 us. It retains the boot trim; no eFuse reload or programming occurs.
`low_frequency_hz()` is nominally 32000 and TIMER Clock1K uses F32K / 32.
RC tolerances and low-frequency domain readiness need measurement.

## Watchdog

`Watchdog::new(p.watchdog, p.clocks, Config)` consumes a separate token, enables
the TIMER gate and configures a stopped watchdog without resetting the block.
Only reset mode (WRIE=1) is implemented; interrupt mode/IRQ54 is not.

Sources are **System (FCLK)** and RC32K, with actual divider 1..256 and match
1..65535. Source code 3 is deliberately omitted: the RM calls it PLL 32 MHz while
SDK variants call it XTAL/32M. `core::time::Duration` is rounded up to microseconds
and then ticks; unrepresentable values are rejected, not truncated. The default
RC32K / 32, 2-second nominal timeout uses match=2000. Actual timing depends on
the oscillator tolerance.

Protected accesses write WFAR=0xBABA and WSAR=0xEB10 under a critical section.
Start resets the counter before enabling WE; feed writes WCR=1. Stop is explicit.
**Drop does not stop the watchdog**: losing the handle does not remove protection.
`counter`, `is_running` and `nominal_timeout` are diagnostics, not proof of reset.
TCCR/TCDR updates preserve both regular timer channels.

## Reset

`Peripherals.reset_reason` is a snapshot taken before clock initialization.
`reset::reason()` and explicit `clear_reason()` are also available. HBN flags for
POR/external/software/power-reset/brownout are independent and may be sticky
simultaneously; watchdog status comes separately from TIMER.WSR. The API does not
infer one exclusive cause. Clearing follows the HBN pulse and WSR write-zero
semantics in the SDK/RM.

`reset::system_reset() -> !` disables MIE, switches to RC32M through ITCM and pulses
system+CPU reset in GLB. It neither erases flash nor switches ROM ISP/boot mode.
A RAM ELF does not automatically restart after reset: ROM and boot flash determine
the next boot path.

The `system` example tries XTAL/DLL144 with cap33, explicitly falls back to RC32M
on XTAL timeout, prints clocks/reset flags, and feeds the watchdog every 100 ms
through an Embassy Timer. The reset function is retained for ELF inspection but
never called. Stopping feed or halting under a debugger may cause watchdog reset
on a future hardware run. GPIO21 is held low after initialization.

## Sources and validation

Compared with the [official SDK](https://github.com/bouffalolab/bouffalo_sdk/tree/237ed937856bf435f4f3642ea4728294723e1d87/drivers/soc/bl702/std/src):
`bl702_glb.c` for clocks/dividers/DLL/system reset, `bl702_aon.c` for XTAL,
`bl702_hbn.c` for RC32K/reset events, `bl702_clock.c` for FCLK/BCLK muxes,
LHAL `bflb_wdg.c`, [RM TIMER registers](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/tmr_register.rst)
and the old SDK/IronOS.

Four host tests cover plans/rates, mux encodings, bounds and watchdog rounding.
MMIO sequences, stability and reset have not executed on hardware. CLIC/mtime,
UART timing, XTAL/DLL, watchdog expiry/flags, shared timer preservation and debug
behavior remain on the [hardware checklist](hardware.md).
