//! XTAL/DLL + watchdog bring-up. No intentional reset is performed by this example.
#![no_std]
#![no_main]
use bl706_hal::{
    clock::{Clocks, Config},
    gpio::Output,
    reset::ResetReason,
    watchdog::{Config as WatchdogConfig, Watchdog},
};
use core::fmt::Write;
use embassy_time::{Instant, Timer};
use pinecil_v2_bringup as _;
use pinecil_v2_bringup::Logger;
use static_cell::StaticCell;
static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();
#[embassy_executor::task]
async fn check(
    mut log: Logger,
    _heater: Output<21>,
    mut watchdog: Watchdog,
    clocks: Clocks,
    reason: ResetReason,
    fallback: bool,
) {
    let _ = writeln!(
        log,
        "BL706 system: root={:?} CPU={} BCLK={} UART={} fallback={fallback} reset={reason:?} watchdog={:?}\r",
        clocks.root(),
        clocks.cpu_hz(),
        clocks.bus_hz(),
        clocks.uart_hz(),
        watchdog.nominal_timeout()
    );
    // Keep system_reset linked for inspection without executing it.
    core::hint::black_box(bl706_hal::reset::system_reset as fn() -> !);
    watchdog.start();
    let mut count = 0u32;
    loop {
        watchdog.feed();
        count = count.wrapping_add(1);
        if count.is_multiple_of(10) {
            let _ = writeln!(
                log,
                "us={} watchdog_counter={} running={}\r",
                Instant::now().as_micros(),
                watchdog.counter(),
                watchdog.is_running()
            );
        }
        Timer::after_millis(100).await;
    }
}
#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let log = Logger::init();
    let (p, fallback) = match bl706_hal::init_with_config(Config::pinecil_v2_144mhz()) {
        Ok(p) => (p, false),
        Err(_) => (bl706_hal::init(), true),
    };
    let heater = p.pins.p21.into_output(false);
    let watchdog = Watchdog::new(p.watchdog, p.clocks, WatchdogConfig::default()).unwrap();
    bl706_hal::enable_interrupts();
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(check(log, heater, watchdog, p.clocks, p.reset_reason, fallback).unwrap());
    })
}
