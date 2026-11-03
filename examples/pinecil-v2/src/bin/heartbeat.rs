#![no_std]
#![no_main]
use core::fmt::Write;
use embassy_time::{Instant, Timer};
use pinecil_v2_bringup::{Board, banner, board};
use static_cell::StaticCell;
static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();

#[embassy_executor::task]
async fn heartbeat(mut b: Board) {
    banner(&mut b.uart, "heartbeat");
    let mut count = 0u32;
    loop {
        let _ = writeln!(
            b.uart,
            "tick={count} us={} keyA={} keyB={} PD_IRQ={}\r",
            Instant::now().as_micros(),
            b.key_a.is_high(),
            b.key_b.is_high(),
            b.pd_irq.is_high()
        );
        b.heater_off.set_low();
        count = count.wrapping_add(1);
        Timer::after_millis(1000).await;
    }
}
#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let b = board();
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(heartbeat(b).unwrap());
    })
}
