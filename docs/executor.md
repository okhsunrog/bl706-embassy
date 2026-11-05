# Embassy thread and interrupt executors

The HAL supplies `bl706_hal::Executor` and `bl706_hal::InterruptExecutor`
using Embassy's raw executor. Applications must depend on `embassy-executor`
with `default-features = false`, without any `platform-*` or `executor-*`
features: the HAL supplies `__pender`. Task macros remain available.

The thread executor retains the previous spin/poll behavior. The interrupt
executor uses machine software interrupt **3**, backed by the 32-bit MSIP at
**0x02000000**. These are the BL702 SDK's definitions in
[e24/clic.h](https://github.com/bouffalolab/bouffalo_sdk/blob/237ed937856bf435f4f3642ea4728294723e1d87/drivers/lhal/include/arch/risc-v/e24/clic.h)
and [bl702.h](https://github.com/Ralim/IronOS/blob/c7413e0d2adfd43156510d4ccbd109cf352ca335/source/Core/BSP/Pinecilv2/bl_mcu_sdk/drivers/bl702_driver/regs/bl702.h).
The older CLIC byte-array layout is different from NMSIS ECLIC and T-Head CLIC.

Store an `InterruptExecutor` in a static, then call
`start(p.software_interrupt)` after HAL initialization. The unique token
reserves this interrupt and prevents multiple owners. `start` publishes fully
initialized handler state before enabling the source and returns a
`SendSpawner`; tasks crossing from thread mode must therefore be `Send`.
Enable global interrupts using the usual `bl706_hal::enable_interrupts()`.

Wakeups only set MSIP; they never poll a future synchronously. The ISR clears
MSIP **before** polling, so a wake during a poll causes another interrupt.
This covers timer/GPIO/I2C wakeups and thread-mode mutex unlocks. Do not invoke
the raw executor or software handler from another context.

## Scheduling and shared state

The CLIC uses one level bit (`cliccfg.nlBits=1`): the software executor runs at
level 127 and all peripheral handlers run at level 255. The runtime saves mepc,
mcause (including MPIL), mstatus and FP state before enabling MIE **only** for
the software handler. Peripheral ISRs can preempt PD, but cannot nest each other;
the software executor cannot reenter itself. Before returning, the handler masks
MIE and restores its saved return state. This follows E24 manual sections 8.4.1
and 8.7.8; see [sources](sources.md).

Keep polls short, use async I/O, and never spin waiting for another task. I2C
has a two-word FIFO: a non-preemptible software handler delayed its service and
caused OLED FIFO errors in the application even at 5 V. Higher-level peripheral
preemption is required for this use case. Long critical sections still block it.

Use `CriticalSectionRawMutex` for Embassy synchronization shared with thread
mode. `NoopRawMutex`, blocking waits and shared `RefCell` state without interrupt
protection are inappropriate across these contexts. Avoid long logging and
interrupt-disabled sections. Do not continuously self-wake an interrupt task:
that can starve thread mode even when each individual poll is short.

The interrupt shares the thread's stack, including the 224-byte IRQ frame.
With `fpu`, the existing entry/exit code saves and restores all 32 single
precision FP registers, FCSR and mstatus at both interrupt levels. No A-extension
instructions are used.

## Validation

`interrupt_executor` is a dedicated HIL example, built for both IMC and IMFC.
It reads nine side-effect-free FUSB302 registers through interrupt-driven I2C
while the thread stays in a CPU loop. It also checks timer wakeups, exclusion
by an async mutex and wakeup when thread mode releases that mutex. With `fpu`,
the software ISR deliberately changes ft11 and FCSR while the busy thread
keeps probe values live in a single assembly block.

ELF audits check vector 3 and the existing full FPU save/restore coverage.
On 2026-11-05, the IMFC RAM example completed 113 consecutive rounds on Pinecil
V2, with 31 successful I2C reads during each non-yielding thread interval.
Mutex exclusion, wakeup on thread unlock, timer wakeups, ft11 and FCSR checks
all passed. The 35-second host command was stopped deliberately; no panic or
I2C failure appeared.

The preemptible version then completed 64 rounds with 48 reads per busy-thread
interval. Its software task additionally stalled *inside* the first I2C future
poll: peripheral ISRs had to drain the FIFO while the software handler was still
running. FP checks passed in both thread and software-interrupt busy loops.
This tests the executor port, not PD scheduling under OLED load; integration in
the application remains a separate hardware check.
