#![no_std]
#![no_main]
use core::fmt::Write;
use embassy_time::Timer;
use pinecil_v2_bringup::{Board, banner, board};
use static_cell::StaticCell;
static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();

#[embassy_executor::task]
async fn probe(mut b: Board) {
    banner(&mut b.log, "FUSB302 read-only probe");
    loop {
        // FUSB302MPX address 0x22 (7-bit, not the legacy shifted 0x44).
        // DEVICE_ID=0x01; avoid INTERRUPT (read-to-clear), RESET and PD TX.
        use bl706_hal::embedded_hal_async::i2c::I2c;
        let mut id = [0u8; 1];
        let result = b.i2c.write_read(0x22, &[0x01], &mut id).await;
        let _ = writeln!(
            b.log,
            "device_id={:02x} result={result:?} IRQ={}\r",
            id[0],
            b.pd_irq.is_high()
        );
        Timer::after_millis(1000).await;
    }
}
#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let b = board();
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(probe(b).unwrap());
    })
}
