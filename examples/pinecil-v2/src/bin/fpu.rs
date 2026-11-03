#![no_std]
#![no_main]
use core::fmt::Write;
use embassy_time::Timer;
use pinecil_v2_bringup::{Board, banner, board};
use static_cell::StaticCell;
static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();

#[embassy_executor::task]
async fn check(mut b: Board) {
    banner(&mut b.uart, "FPU");
    loop {
        let x = core::hint::black_box(1.5f32);
        let y = core::hint::black_box(2.0f32);
        let result = x * y + 0.25;
        let mstatus: usize;
        let fcsr: usize;
        unsafe {
            core::arch::asm!("csrr {}, mstatus", out(reg) mstatus);
            core::arch::asm!("frcsr {}", out(reg) fcsr);
        }
        let _ = writeln!(
            b.uart,
            "f32 bits={:08x} expected=40500000 FS={} fcsr={fcsr:08x}\r",
            result.to_bits(),
            (mstatus >> 13) & 3
        );
        assert_eq!(result.to_bits(), 0x4050_0000);
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
