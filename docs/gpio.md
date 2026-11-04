# GPIO and async interrupts

`Input<N>` implements `embedded_hal::digital::InputPin` and
`embedded_hal_async::digital::Wait`. All five waits use GPIO interrupts, without
timer polling:

```rust,ignore
let mut int_n = p.pins.p16.into_input(bl706_hal::gpio::Pull::Up);
int_n.wait_for_low().await;
// Service the source and release INT_N before waiting for low again.
```

Low/high waits finish immediately if the level is already active. Rising/falling/
any-edge waits require a new event after detector arming on the first poll; stale
pending bits are cleared. Pre-arming events are not retained. `Wait` returns
`Infallible`; `&mut self` and exclusive pin ownership allow one waiter per input.

## Embassy pattern

Compared with Embassy commit `4c0da6097c5b3ce078473016d7c4654f90693afa`:

- [embassy-rp GPIO](https://github.com/embassy-rs/embassy/blob/4c0da6097c5b3ce078473016d7c4654f90693afa/embassy-rp/src/gpio.rs):
  per-pin wakers, stale edge clearing before arming, ISR masking and task wakeup.
- [embassy-stm32 EXTI](https://github.com/embassy-rs/embassy/blob/4c0da6097c5b3ce078473016d7c4654f90693afa/embassy-stm32/src/exti/mod.rs):
  per-line wakers, one-shot masking and cancellation cleanup. STM32 needs separate
  ExtiInput because ports share EXTI channels. BL706 has no such cross-port
  contention, so waits are part of `Input`, as in RP.

Per-pin wakers and `armed/complete` bitsets live in static memory under one critical
section. No Input/future pointer reaches the ISR. Waker registration and completion
checks are serialized with the ISR; an early event remains latched. Triggered pins
are masked in **both detectors**, status is cleared and wakers are extracted before
being called outside the state borrow. One IRQ can complete several independent waits.

Future Drop, including timeout/select, masks both detectors for that pin, clears
pending/completion and releases its waker. Other pins keep waiting. Input Drop does
the same. A forgotten future leaves no dangling pointer; it stays armed until an
event, rearming or Input Drop. Events mask it once; a new wait clears old state.

## BL706 registers

Sources: [RM GLB](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/GLB.rst),
[register tables](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/RST/glb_register.rst),
[SDK GLB](https://github.com/bouffalolab/bouffalo_sdk/blob/237ed937856bf435f4f3642ea4728294723e1d87/drivers/soc/bl702/std/src/bl702_glb.c)
(`GLB_Set_GPIO_Int[2]Mod`, `GPIO_INT0_IRQHandler`) and
[LHAL GPIO](https://github.com/bouffalolab/bouffalo_sdk/blob/237ed937856bf435f4f3642ea4728294723e1d87/drivers/lhal/src/bflb_gpio.c).

- Shared CLIC source `GPIO_INT0` is 16+44 = **60**. HAL installs its vector/dispatcher,
  masks all GPIO sources, clears both detectors and enables the CLIC line at init.
  The application still enables global MIE afterward.
- Detector 1: MASK `0x1a0`, STAT `0x1a8`, CLR `0x1b0`, MODE `0x1c0..0x1cc`.
  Detector 2: MASK `0x1d0`, STAT `0x1d4`, CLR `0x1d8`, MODE `0x1dc..0x1e8`.
  MASK=1 disables the source; both STAT registers are RO. CLR is RW and pulsed
  1→0 by the SDK, not W1C; the PAC annotations reflect this.
- Ten logical GPIOs per MODE word, three bits per pin. RMW under a critical section
  preserves neighbors. SF/PSRAM remap affects pad config, not IRQ bit numbering.
- SDK async modes are falling=4, rising=5, low=6, high=7. Any-edge uses rising in
  detector 1 and falling in detector 2. SDK ISR services both STAT registers in
  GPIO_INT0; their independence/routing still require hardware confirmation.
  The driver does not switch polarity based on a sampled level.
- Level waits recheck the level after unmasking. The ISR masks first, so a sustained
  low/high should not cause an IRQ storm.

## Validation limits

Seven host tests run the production state engine with modeled registers: all 32
mode fields, edge directions, early latched completion, active levels, level/unmask
races, both detectors, multiple pins, cancel/stale IRQ/rearm. They do not execute
target MMIO/waker/future Drop glue or prove electrical GPIO operation, CLR pulse
width, latency, minimum input pulse width or return from IRQ60.

This is one-shot notification, not an edge counter. Pulses between waits are lost
and software debounce is absent. Async control does not imply deep-sleep wakeup:
the executor spins and HBN/PDS GPIO wakeup is unimplemented.

`gpio_irq` demonstrates GPIO25 any-edge with timeout/rearm, GPIO28 rising/falling,
GPIO16 FUSB302 low/high and an independent UART/timer reporter. It observes INT_N
without clearing FUSB302 interrupt registers or implementing PD policy. A stuck-low
INT_N leaves that task waiting for high while others continue. Button bounce can
increment counts repeatedly. The example has not run on hardware; see [the checklist](hardware.md).
