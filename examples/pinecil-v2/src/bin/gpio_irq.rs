#![no_std]
#![no_main]
use bl706_hal::{
    gpio::{Input, Output},
    portable_atomic::{AtomicU32, Ordering},
    uart::Uart0,
};
use core::fmt::Write;
use embassy_time::{Duration, Timer, with_timeout};
use pinecil_v2_bringup::{Board, banner, board};
use static_cell::StaticCell;

static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();
static A_EDGES: AtomicU32 = AtomicU32::new(0);
static A_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
static B_PRESSES: AtomicU32 = AtomicU32::new(0);
static PD_ASSERTS: AtomicU32 = AtomicU32::new(0);

#[embassy_executor::task]
async fn key_a(mut pin: Input<25>) {
    loop {
        // Exercise the standard trait and cancellation/rearm every three seconds.
        let result = with_timeout(
            Duration::from_secs(3),
            bl706_hal::embedded_hal_async::digital::Wait::wait_for_any_edge(&mut pin),
        )
        .await;
        if result.is_ok() {
            A_EDGES.fetch_add(1, Ordering::Relaxed);
        } else {
            A_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
        }
    }
}
#[embassy_executor::task]
async fn key_b(mut pin: Input<28>) {
    loop {
        pin.wait_for_rising_edge().await;
        B_PRESSES.fetch_add(1, Ordering::Relaxed);
        pin.wait_for_falling_edge().await;
    }
}
#[embassy_executor::task]
async fn pd_irq(mut pin: Input<16>) {
    loop {
        pin.wait_for_low().await;
        PD_ASSERTS.fetch_add(1, Ordering::Relaxed);
        // Observation only: no FUSB302 read-to-clear registers or PD writes.
        // If INT_N remains asserted this task stays here, without an IRQ storm.
        pin.wait_for_high().await;
    }
}
#[embassy_executor::task]
async fn report(mut uart: Uart0, _heater_off: Output<21>) {
    banner(
        &mut uart,
        "GPIO IRQ: keys A/B and FUSB302 INT_N (no debounce)",
    );
    loop {
        let _ = writeln!(
            uart,
            "A_edges={} A_timeouts={} B_presses={} PD_asserts={}\r",
            A_EDGES.load(Ordering::Relaxed),
            A_TIMEOUTS.load(Ordering::Relaxed),
            B_PRESSES.load(Ordering::Relaxed),
            PD_ASSERTS.load(Ordering::Relaxed)
        );
        Timer::after_secs(1).await;
    }
}
#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let Board {
        uart,
        heater_off,
        key_a: a,
        key_b: b,
        pd_irq: pd,
        ..
    } = board();
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(key_a(a).unwrap());
        s.spawn(key_b(b).unwrap());
        s.spawn(pd_irq(pd).unwrap());
        s.spawn(report(uart, heater_off).unwrap());
    })
}
