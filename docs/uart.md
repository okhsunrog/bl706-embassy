# UART0

`Uart0::new(token, gpio22, clocks, baud)` retains the blocking TX-only API and
`core::fmt::Write`. `AsyncUart0` adds interrupt-driven TX/RX with
`embedded-io-async` 0.7 traits. Both use 8 data bits, no parity, one stop bit,
LSB first. UART1, alternate pins, DMA, RTS/CTS and configurable frame formats
are outside this implementation. Hardware operation has **not** been tested.

## API

```rust,ignore
use bl706_hal::{embedded_io_async::Write, uart::AsyncUart0};

let p = bl706_hal::init();
let mut uart = AsyncUart0::new(
    p.uart0, p.pins.p22, p.pins.p23, p.clocks, 115_200,
).unwrap();
bl706_hal::enable_interrupts();

uart.write_all(b"hello\r\n").await.unwrap();
uart.flush().await.unwrap();
let mut buffer = [0; 32];
let n = uart.read(&mut buffer).await.unwrap();
```

`read` returns once at least one byte is available; `write` queues a nonzero
prefix when space is available. Both return zero for an empty buffer. Use the
trait's `write_all` / `read_exact` helpers to loop over partial progress.
`flush` waits for both FIFO empty and the last character leaving the shift
register. Writes do not implicitly flush.

`split(&mut self)` returns borrowed `UartTx` and `UartRx` halves, allowing one
read and one write/flush at the same time. Each half implements its respective
trait. The parent stays borrowed until both halves are released. A static
parent in `StaticCell` permits moving the halves into separate Embassy tasks;
see `examples/pinecil-v2/src/bin/uart_async.rs`.

No internal timeout is imposed on async I/O. Wrap operations in
`embassy_time::with_timeout` when a deadline is needed. Global interrupts and
a running Embassy time driver are required.

## IRQ, cancellation and errors

UART0 is CLIC source 45. Pending RX FIFO/parity/FIFO-error and TX FIFO/FIFO-error
sources mask their own direction before waking its waiter. Tasks move at most
one FIFO worth of bytes per poll; the ISR retains only wakers and never holds
caller buffer pointers. Register inspection, subscription changes and FIFO
access use the single-hart critical section, including concurrent split halves.
A level FIFO condition arising between the count read and unmask is still
pending; IRQ-before-next-poll is handled by checking hardware again.

Dropping a pending read/write/flush masks that direction and releases its waker,
without clearing either FIFO or affecting the other half. A pending single
`read` or `write` has consumed/queued no bytes. Already queued TX bytes continue
to transmit after cancellation. Cancelling `write_all` or `read_exact` may leave
a transferred prefix; retrying the complete message can duplicate bytes or lose
framing. Cancelling flush does not retract TX. Forgetting a future retains no
caller pointers; a stale subscription can wake once before masking itself.
Dropping the parent masks/disables UART0 IRQs, disables TX/RX and clears FIFOs;
flush first if queued output must be preserved.

RX FIFO error returns `Overrun`, parity error returns `Parity`; either discards
the RX FIFO so a later read can resume. The failing read leaves its destination
unchanged. TX FIFO error returns `Fifo` and clears the TX FIFO, potentially
losing queued output. No line framing/break detection is claimed in normal UART
mode; LIN synchronization errors are not enabled. A stream protocol must provide
its own resynchronization after errors.

RX uses only the **128-byte hardware FIFO**, with threshold zero. It is not a
background buffered receiver: sustained reception requires timely task polls,
even after the first RX IRQ masks itself. At 115200 8N1, a full FIFO represents
about 11 ms of data. Long task polls/critical sections may cause overruns; no
software ring buffer or flow control prevents them. TX similarly may have gaps
if the task is delayed after the FIFO empties.

Free-run explicitly disables TX_END according to the RM. Flush therefore arms
the TX FIFO request at threshold 127 (empty), then checks `sts_utx_bus_busy`,
yielding through Embassy Timer for one nominal character time between checks.
It never enables a continuously pending empty-FIFO IRQ while waiting for the
shift register. Clock/divider calculations use overflow-safe integer arithmetic;
bit timing still depends on the actual clock accuracy.

## Sources

Reviewed against the pinned [BL70x RM1.2](https://github.com/bouffalolab/bl_docs/blob/665e7daa84b498a767e656f458f26aca3be4fd45/BL702_RM/en/BL702_BL704_BL706_RM_en_1.2.pdf),
chapter 10: FIFO request conditions, free-run/TX_END, FIFO reset count 128,
seven-bit thresholds, BUS_BUSY and FIFO-clear/error semantics; and the official
[SDK UART driver](https://github.com/bouffalolab/bl_iot_sdk/blob/edcf613f073d3ef7d2f290d7c285b69be35da75b/components/platform/soc/bl702/bl702_std/BSP_Driver/std_drv/src/bl702_uart.c)
and header (`UART_TX_FIFO_SIZE` / `UART_RX_FIFO_SIZE` = 128).
The RM feature list says 32 bytes, conflicting with its functional description,
FIFO reset-count register and SDK constants. This implementation follows the
latter 128-byte model; actual FIFO depth and threshold behavior need board checks.
GPIO22/23 mux selection follows the [SDK GLB definitions](https://github.com/bouffalolab/bouffalo_sdk/blob/237ed937856bf435f4f3642ea4728294723e1d87/drivers/soc/bl702/std/include/bl702_glb.h).

## Offline validation and future hardware checks

`tools/check.sh` runs the host UART engine tests and includes `uart_async` in
IMC/IMFC builds, clippy and ELF audits, including the IRQ45 vector. Host tests
cover partial FIFO transfers, data-before-poll, arrival during IRQ arming,
one-shot wakes, independent cancellation, preserved TX/RX data, error/reuse,
empty buffers, baud bounds and FIFO-drain threshold switching. These checks do
not establish physical UART operation.

When hardware testing is explicitly requested, use a 3.3 V UART adapter with
common GND, GPIO22 TX to adapter RX and GPIO23 RX from adapter TX. Do not connect
RS-232 voltage levels. A TX-to-RX loopback can exercise concurrent halves.
`uart_async` holds GPIO21 low and keeps diagnostic output on RTT. Verify 8N1
bit time, FIFO depth/thresholds, byte order for lengths1/127/128/129/320, single
bytes after idle, independent TX/RX, idle timeout/cancel/rearm and no IRQ storm.
Observe the final stop bit before flush returns, including a single-byte write.
Check overflow/error recovery under a deliberately delayed RX task and verify
RTT/timers continue throughout. No such hardware tests were performed here.
