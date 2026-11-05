# ADC, PWM and TIMER

ADC has been exercised on Pinecil V2's BL706 through
[`pinecil-rs`](https://github.com/okhsunrog/pinecil-rs). TIMER0 free-running compare
callbacks have also been tested with PWM1 at zero duty and with GPIO21's 50%
carrier during bounded heating tests. Other PWM/TIMER paths still need hardware
qualification. Times and voltages below are nominal. The
standalone examples keep GPIO21 low with PWM unrouted; application heater
regulation and its qualification belong to the board firmware.

## Ownership and Embassy

`Peripherals` supplies one `AdcToken`, two `TimerToken<0/1>` and five
`PwmToken<0..4>`. Constructors consume them. ADC owns analog pads; PWM owns its
output pad or `Unrouted`. A TIMER channel has one shared counter and three compares,
not three independently owned timers. Shared TCCR/TCDR/TCER/TCMR fields and clock
gates are updated under critical sections, preserving the adjacent channel and
watchdog fields. The whole TIMER block is never reset. The watchdog has its own
[token and driver](system.md).

`embassy_time` uses core-local `mtime`, leaving TIMER0/1 available. Async APIs require
global MIE enabled. Static wakers/IRQ state retain no future or caller-buffer pointers.
Completion before waker registration stays latched. Completion/cancellation masks
the source, clears status and releases the waker. CLIC sources: ADC=41, PWM=50,
TIMER0=52, TIMER1=53. Peripheral handlers share one CLIC level, so a long handler
delays its peers. They can preempt the lower-level software interrupt executor.

## ADC

Finite single-ended scans contain 1..12 entries, including repeated channels.
Inputs are GPIO18/CH8, GPIO19/CH9, GPIO20/CH10 and internal GND/CH23. Analog pads use
function10 without IE/SMT/pulls/OE. `Inputs` accepts one analog pin, pairs, triples,
nested tuples or `()` for GND. A scan cannot use an external channel unless its pad
was passed to ADC. Other external inputs, differential mode, TSEN/VBAT, audio,
continuous scans and DMA have no public API yet.

```rust,ignore
let adc_inputs = (p.pins.p18.into_analog(), p.pins.p19.into_analog(),
                  p.pins.p20.into_analog());
let trim = GainCalibration::from_efuse_shadow();
let mut adc = Adc::new(p.adc, adc_inputs, p.clocks, Config {
    calibration: trim.ok(), ..Default::default()
});
let mut samples = [Sample::default(); 3];
adc.read_scan(&[Channel::Input8, Channel::Input9, Channel::Input10],
              &mut samples, Duration::from_millis(100)).await?;
```

ADC uses XCLK32M / GLB4 / internal4 = nominal 2 MHz, within the RM maximum.
XCLK can be RC32M or XTAL32M under the selected clock profile, independently of
DLL CPU rate. GPIP (`cgen_cfg1[2]`) and HBN GPADC gates are enabled. The divider is
derived from `Clocks`. Analog settings match the Pinecil BSP: V18=1.82 V, V11=1.1 V,
PGA1/PGA2 x1, chop=2, main bandgap, VCM=1.2 V, delay=2 and offset correction off.
Modern LHAL also enables MIC2_DIFF/NEG_GND and performs other calibration; that
sequence was not copied blindly because IronOS explicitly leaves MIC2_DIFF off.
The Pinecil measurements below exercise this configuration; absolute accuracy
still needs reference measurements and calibration.

Resolutions are 12-bit, 14-bit average16/64 and 16-bit average128/256: averaged
12-bit SAR results, not a claim of physical 16-bit accuracy. FIFO words contain
sample[15:0], negative tag[20:16] and positive tag[25:21]. `raw` retains left alignment;
`counts` is right-aligned. The driver checks positive-tag order, negative tag=GND,
extra data, overrun/underrun and analog saturation. Negative tag=23 was observed
on Pinecil V2; other modes and channels remain outside that hardware test.

`GainCalibration::from_efuse_shadow` only reads the boot-loaded shadow and enables
the EF_CTRL AHB gate. It does not reload, invoke ROM APIs or write EF_CTRL/EF_DATA/
fuses. An unavailable/busy/non-SAHB shadow returns `ShadowUnavailable`; absent trim
or invalid parity returns `InvalidCalibration`. EF_DATA_0 +0x78 contains trim[12:1],
parity[13], enable[14]. The SDK integer formula is
`counts * 2048 / (2048 - signed_trim12)`, saturated to the resolution maximum.
`calibration=None` produces uncalibrated counts and `Sample.calibrated=false`.
Log the shadow result: fallback is not proof of calibration. `microvolts()` estimates
ADC-pin voltage without VIN divider, thermocouple amplifier, NTC or reference errors.

Async scans use FIFO IRQ threshold=1 word and drain at most 12 results per entry.
The ISR stops ADC and masks IRQ before publishing completion. The future only waits
and copies successful results; caller output is unchanged on error/timeout/cancel.
Errors are checked before and after draining. GPIP has mixed status/control: mask
writes do not echo W1C flags. FIFO_CLR/error clears are explicit cleanup writes.
The RM scan description mentions CONT_CONV_EN=1, whereas SDK/IronOS finite scans
use SCAN_EN=1 and CONT_CONV_EN=0. This driver follows that SDK path. Four-entry
scans of repeated CH8, CH10 and CH9 completed on Pinecil; mixed-channel ordering
and other scan lengths still need hardware tests.

Manual ISR-driven operation uses `prepare_scan`, `start_prepared`, `stop`, `try_read`.
Preparation stops ADC and clears FIFO. `start_prepared` never waits; it returns
Busy if ADC is running or 100 us have not elapsed after preparation/stop. The pause
comes from SDK `ADC_RESTART_DUMMY_WAIT`; async scans wait via Embassy Timer.
A shared restart gate records every actual STOP, including ISR completion/error
and drop/cancel, so switching to manual start cannot bypass it. `stop` preserves
FIFO for the owner to drain; `abort` clears FIFO and async state, including forgotten
futures. A forgotten scan occupies the engine until completion or abort. If forgotten
during the pre-START 100 us pause, it never starts and stays Busy until abort.
ADC Drop stops conversion and clears GLOBAL_EN; it does not return the pins.

## PWM

Five independent channels support BCLK, actual divider 1..65535, period 1..65535,
normal polarity and abrupt stop. Rate = BCLK / divider / period, with no hidden +1.
Function8 routes GPIO modulo5 to the corresponding channel; only use an available
board pad. `Unrouted` tests counter/IRQ behavior without a physical output.

`embedded_hal::pwm::SetDutyCycle` reports max duty=period, rejects out-of-range duty
and uses SW force for 0/100%. Construction disables/forces low before routing a pad;
`enable()` is explicit. `disable()` forces low before STOP_EN. Drop also masks IRQ
and returns the pad to GPIO low. `configure` disables PWM and waits for STS_TOP for
nominally 1 ms, with a hard poll budget if mtime fails; IRQs stay enabled. Live duty
changes are not promised to be phase-synchronous or glitch-free.

`wait_periods(n)` waits for the next hardware period-count IRQ at threshold=n
without changing duty. Arming does not reset counter phase, so it is not an exact
delay of n full periods. The shared IRQ services channels independently and masks
triggered ones once. Clear pulses 1→0, with up to 32 intervening status reads per
SDK; failure to clear returns Timeout. Cancelling masks IRQ/releases the waker but
does not stop PWM. Forgotten futures retain no caller references; IRQ stays one-shot.

## TIMER

The TCDR divider fields for logical TIMER0/1 are the hardware TIMER2/3
fields at bits 8..15 and 16..23. Bits 0..7 are reserved; bits 24..31 belong
to the watchdog. The driver uses the named PAC setters to preserve them.

`InterruptDriver::with` provides bounded, interrupt-masked access to the same
timer and handler used by the ISR. This allows an application to arm a single
heater pulse, while the ISR owns its cutoff. Callbacks and `with` closures
must not wait, log or reenter the same driver. Static registration shares the
driver through its critical-section mutex without aliased mutable references.

Each channel has a 32-bit counter, actual divider 1..256 and three raw compares,
with free-running or preload mode triggered by one compare. Sources are System
(FCLK from the selected profile), 32K and 1K. Init explicitly powers RC32K and selects
it for F32K; low-frequency rates remain nominal 32000/1000, not measured readiness
or calibration. The ambiguous source-code3 XTAL/PLL32M path is not exposed.

Compare values are written as given. SDK init subtracts two ticks in some cases,
while its setter writes raw values. This ambiguity is explicit: measure actual
intervals/preload boundaries. `counter` reads the continuously synchronized
TCVSYN2/3 register. The SDK's TCVWR capture request plus three APB reads returned
the previous latch on the BL706 at CPU 144 MHz and TIMER0 1 MHz. A second pulse
was armed at comparator 2046 while the counter had passed 30000; its new capture
latch later contained 30245. Three APB reads are not a capture-completion
handshake. TCVSYN avoids that request and may lag by clock-domain latency.
Stop does not promise counter reset; count/preload/restart need HIL.
A compare already behind count can wait until rollover, rather than matching at once.

`Timer<N>` supplies start/stop/configure/set_compare/listen/take_events. Without a
custom ISR, its handler masks matched compares and latches events. `wait_for_compare`
waits for a new match on a running counter and clears old pending on arming.
Cancellation masks its compare without stopping the counter. Repeated matches can
coalesce; this is not a queue of every event.

Alternatively, `InterruptDriver<N,H>` consumes Timer and `H: Handler<N>`. Place it
in StaticCell; `register(&'static self, events)` permanently registers the shared
driver with the runtime. ISR acknowledges events and directly calls
`H::on_interrupt(&mut self, &mut Timer<N>, Events)`. H may own ADC/PWM and control
them without waking the executor. The application no longer owns a separate Timer,
preventing two handlers sharing the channel. Callbacks must not log, block or wait
for another IRQ. All pending compares arrive in one Events value; the order of
missed phases cannot be reconstructed from these bits under overload.
The application may use `with` for short synchronized access to the owned timer
and handler; a critical-section mutex prevents aliasing the ISR's mutable access.

## Examples and limits

- `analog`: eight-entry async ADC scan, TIMER1 compare and PWM0 period IRQ with
  timeout; PWM0 unrouted, GPIO21 low. Logs gain-trim status and raw samples.
- `sampling`: static ISR driver owns TIMER0, ADC and **unrouted PWM1**. Compare0=50000
  disables PWM counter output; compare1=51000 starts ADC; compare2=60000 stops/drains,
  prepares the next scan and resumes unrouted PWM. Nominal 1 MHz clock, preload on
  C2. Coalesced/unexpected events, late phases or ADC errors stop this dry run.
  A separate task logs the latest Frame through Embassy Signal. Signal can replace
  unread frames; it is not lossless. The heater remains GPIO low throughout.

Host tests cover the production ADC engine, lengths 1..12, errors before/after drain,
cancel/reuse, packing, resolution, all signed12 gain trims/parity, PWM/TIMER bounds
and preservation of adjacent fields. They do not verify MMIO, actual tags/W1C,
callbacks/waker glue, IRQ latency, analog settling, output waveforms or fail-off timing.
`sampling` is not a regulator: command-freshness cutoff, PD/power limits, thermocouple
conversion and qualified heating protection are absent. See [hardware.md](hardware.md).

## ADC hardware evidence, 2026-11-05

Pinecil V2 ran the XIP/FPU `pinecil-rs` image with GPIO21 held low and no PWM
output routed. Its read-only task sampled VIN/handle/tip at 4 Hz using independent
four-entry scans, 14-bit average16 and a 25 ms timeout per scan. The eFuse shadow
provided a valid gain coefficient with denominator 1981.

The first run timed out because the runtime used peripheral number 25 directly
as the interrupt number. The RISC-V SDK adds `IRQ_NUM_BASE=16`: ADC is CLIC41,
and the same audit identified PWM as CLIC50. The
[routing correction](https://github.com/okhsunrog/bl706-embassy/commit/a2cc082defd0e35cdcecb342dd2df4c4e507834b)
updates generated PAC metadata, dispatch, vectors and ELF audits together.

After this correction, successive async scans completed without an ADC error.
A 20-second steady-state window reported VIN 20.009..20.052 V and nominal
handle temperature 29.97..30.12 C. An independent simultaneous KM003C capture
contained 19,986 samples with no missing samples and VBUS 20.084..20.095 V.
Mean VIN differed by about 0.3%; this is a comparison at one operating point,
not a calibration. OLED FIFO errors recovered earlier in the session; no such
error appeared in this steady-state window.

This test validates completion of the exercised ADC scans and the application
measurement path. It does not validate all ADC modes, mixed-channel ordering,
fault/cancellation timing on hardware, absolute thermocouple accuracy, PWM,
TIMER callbacks or heater fail-off behavior.

On 2026-11-06, Pinecil's timer-owned output test exposed a stale counter latch:
the SDK-style TCVWR request/three-read sequence failed to rearm a second cutoff.
With TCVSYN, consecutive 20000/2000 us zero-duty windows measured 20044/2003 us.
The second cutoff ran during a 6000 us task-polling stall; both callbacks
completed, with no cancellation. These are firmware timestamps, not a scope
measurement. They qualify this TIMER0 free-running compare/handler path, not
preload/restart modes, period-count PWM interrupts or all clock sources.
The application also observed a reboot after withholding watchdog feeding
with output stopped, and reprogramming with its watchdog armed passed readback
verification. See the [application hardware record](https://github.com/okhsunrog/pinecil-rs/blob/main/docs/heating-hardware.json)
for the image revision, reset-flag limitation and remaining heater tests.

Subsequent Pinecil tests routed PWM1 to GPIO21, with a nominal 5.33 kHz / 50%
carrier and TIMER0 ending each requested on-window. The application completed
532 live bursts with no cancellation and a maximum recorded cutoff lateness
of 112 us. Bounded nominal 5 W / 80 C and 10 W / 150 C requests returned OFF
automatically; injected sensor and stale-frame faults prevented further bursts.
ADC, OLED, buttons, PD and BLE remained operational in the exercised scenarios.
An independent 1000 SPS KM003C recording observed 4.759 A peak current magnitude
under the negotiated 28 V / 5 A contract. PWM waveform timing has not been
measured externally; these observations do not qualify other PWM channels,
duties, period-count interrupts or all timer clock/preload modes.

## Sources

- [RM ADC](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/ADC.rst),
  [ADC registers](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/adc_register.rst),
  [PWM](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/PWM.rst),
  [TIMER](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/TIMER.rst).
- [Official bl_iot_sdk](https://github.com/bouffalolab/bl_iot_sdk/tree/edcf613f073d3ef7d2f290d7c285b69be35da75b/components/platform/soc/bl702/bl702_std/BSP_Driver/std_drv):
  ADC restart/scan/trim, PWM stop/clear, TIMER capture/dividers/compare.
- [SDK LHAL](https://github.com/bouffalolab/bouffalo_sdk/tree/237ed937856bf435f4f3642ea4728294723e1d87/drivers/lhal/src):
  bflb_adc.c, bflb_pwm_v1.c, bflb_timer.c; clock gates in bl702_glb.c.
- [IronOS Setup/IRQ](https://github.com/Ralim/IronOS/tree/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2):
  PGA/chop/averaging, ADC pins, PWM1 and three TIMER0 phases. IronOS firmware policy
  is neither copied nor claimed to have been ported.
