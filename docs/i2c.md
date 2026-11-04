# I²C: servicing the FIFO in the ISR

BL70x RX/TX FIFOs hold two 32-bit words. With threshold=0, FIFO readiness must be
serviced without waiting for the next Embassy task poll. A long cooperative task,
including UART logging, must not stop transfers between the controller and driver
memory.

## Ownership and transfer flow

One static 256-byte packet buffer and `Idle → Active → Complete(Result)` state
are protected by a single-hart critical section. The ISR retains no references or
pointers to caller buffers.

1. Under the critical section, check software ownership and BUS_BUSY, clean the
   controller and configure packet length/sub-address/direction.
2. Copy TX into internal storage and preload at most two words before START.
   For RX, set the expected length. Configure masks, enable CLIC and master.
3. Service at most two FIFO words per ISR entry. Mask FIFO-ready once all bytes
   have entered TX FIFO or left RX FIFO, keeping END/error enabled.
4. Errors take precedence over END. On completion, disable CLIC/controller, clear
   status/FIFO, publish the result and wake the task. Intermediate FIFO IRQs do
   not wake the task or require future polls.
5. The future registers its waker and checks the stored result. Copy successful
   RX into the caller buffer in full; leave that buffer unchanged on error/cancel.

Completion before the waiter's first poll remains latched. END arriving between
an RX FIFO check and the next status read stays enabled for another bounded IRQ,
allowing the final word to be drained without a false `ShortTransfer`.

## Cancellation and forgotten futures

Drop guard and timeout clean the controller and return to Idle in one critical
section. A stale pending IRQ cannot access old transfer data. Hardware completion
also stops the controller directly in the ISR, even if the task is not polled.

Safe Rust does not guarantee Drop: `mem::forget` can forget a future. A borrowed
buffer pointer retained by the ISR would therefore be unsafe if protected only by
a Drop guard. The internal buffer prevents access to freed/reused caller memory.
A new request returns `BusBusy` while a forgotten transfer is Active; after hardware
completion, the internal buffer can be reused.

The cost is 256 bytes of static RAM plus metadata and TX/RX copies. Packets remain
limited to 1..256 bytes. This implementation uses neither DMA nor borrowed zero-copy
state.

## Validation

Host tests run the production transfer engine with a two-word FIFO. They cover TX/RX
for every length 1..256 without repeated waiter polls, empty/full FIFO, partial words,
END/error races, late final RX words, cancellation, stale IRQ and reuse.

`i2c_latency` reads nine FUSB302 bytes at 0x01..0x09. After its first Pending, it
intentionally blocks inside poll for 20 ms with MIE enabled. The ISR must advance
the transfer. Hardware validation must check `stalled=true`, success, data and the
analyzer trace. The example has only been compiled, never run on a board.

The ISR removes dependence on cooperative scheduling, not on long IRQ masking,
other nonnested handlers or hardware delays. Worst-case latency and controller
behavior remain hardware tests.
