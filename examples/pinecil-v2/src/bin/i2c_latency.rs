//! HIL regression: keep the executor inside poll() while IRQs service a read
//! longer than the two-word FIFO. This program has not run on hardware yet.
#![no_std]
#![no_main]
use core::{
    fmt::Write,
    future::{Future, poll_fn},
    task::Poll,
};
use embassy_time::Timer;
use pinecil_v2_bringup::{Board, banner, board};
use static_cell::StaticCell;

static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();

#[embassy_executor::task]
async fn check(mut b: Board) {
    use bl706_hal::embedded_hal_async::i2c::I2c;
    banner(&mut b.log, "I2C IRQ progress with executor stalled");
    loop {
        // Read DEVICE_ID..CONTROL3 (0x01..0x09): all readable registers,
        // no read-to-clear INTERRUPT/FIFO access or PD configuration writes.
        // Nine bytes exceed the two 32-bit FIFO words.
        let mut registers = [0u8; 9];
        let mut stalled = false;
        let result = {
            let mut transfer = core::pin::pin!(b.i2c.write_read(0x22, &[0x01], &mut registers));
            poll_fn(|cx| {
                let result = transfer.as_mut().poll(cx);
                if result.is_pending() && !stalled {
                    stalled = true;
                    let start = bl706_hal::time::now_ticks();
                    // Deliberately do not yield or poll the I2C future again.
                    // MIE stays enabled: the ISR must drain RX during this time.
                    while bl706_hal::time::now_ticks().wrapping_sub(start) < 20_000 {
                        core::hint::spin_loop();
                    }
                }
                match result {
                    Poll::Ready(result) => Poll::Ready(result),
                    Poll::Pending => Poll::Pending,
                }
            })
            .await
        };
        let _ = writeln!(
            b.log,
            "stalled={stalled} result={result:?} registers={registers:02x?}\r"
        );
        Timer::after_millis(1000).await;
    }
}

#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let b = board();
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(check(b).unwrap());
    })
}
