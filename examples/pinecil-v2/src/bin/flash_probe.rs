//! Read-only SPI NOR probe. No writable region is constructed or modified at runtime.
#![no_std]
#![no_main]
use bl706_hal::{
    flash::{Error, Flash, Probe},
    gpio::Output,
};
use core::{
    fmt::Write,
    future::Future,
    task::{Context, Poll},
};
use embassy_time::Timer;
use pinecil_v2_bringup as _;
use pinecil_v2_bringup::Logger;
use static_cell::StaticCell;
static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();

// These wrappers are linked/audited but NEVER called by __bl706_main or the task.
// They type-check both trait surfaces and retain their machine code for the RAM/ISA audit.
#[unsafe(no_mangle)]
fn bl706_flash_blocking_read(flash: &mut Flash, offset: u32, data: &mut [u8]) -> Result<(), Error> {
    embedded_storage::nor_flash::ReadNorFlash::read(flash, offset, data)
}
#[unsafe(no_mangle)]
fn bl706_flash_blocking_write(flash: &mut Flash, offset: u32, data: &[u8]) -> Result<(), Error> {
    embedded_storage::nor_flash::NorFlash::write(flash, offset, data)
}
#[unsafe(no_mangle)]
fn bl706_flash_blocking_erase(flash: &mut Flash, from: u32, to: u32) -> Result<(), Error> {
    embedded_storage::nor_flash::NorFlash::erase(flash, from, to)
}
#[unsafe(no_mangle)]
fn bl706_flash_async_surface(flash: &mut Flash, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
    let mut future = core::pin::pin!(async {
        let mut data = [0u8; 4];
        embedded_storage_async::nor_flash::ReadNorFlash::read(flash, 0, &mut data).await?;
        embedded_storage_async::nor_flash::NorFlash::write(flash, 0, &[0; 4]).await?;
        embedded_storage_async::nor_flash::NorFlash::erase(flash, 0, 4096).await
    });
    future.as_mut().poll(cx)
}
#[embassy_executor::task]
async fn check(mut log: Logger, _heater: Output<21>, mut probe: Result<Probe, Error>) {
    core::hint::black_box(
        bl706_flash_blocking_read as fn(&mut Flash, u32, &mut [u8]) -> Result<(), Error>,
    );
    core::hint::black_box(
        bl706_flash_blocking_write as fn(&mut Flash, u32, &[u8]) -> Result<(), Error>,
    );
    core::hint::black_box(
        bl706_flash_blocking_erase as fn(&mut Flash, u32, u32) -> Result<(), Error>,
    );
    core::hint::black_box(
        bl706_flash_async_surface as fn(&mut Flash, &mut Context<'_>) -> Poll<Result<(), Error>>,
    );
    loop {
        match &mut probe {
            Ok(bus) => {
                let mut header = [0u8; 16];
                let result = bus.read(0, &mut header);
                let _ = writeln!(
                    log,
                    "flash={:?} read={result:?} header={header:02x?}\r",
                    bus.chip()
                );
            }
            Err(error) => {
                let _ = writeln!(log, "flash probe failed: {error:?}\r");
            }
        }
        Timer::after_secs(1).await;
    }
}
#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let log = Logger::init();
    let p = bl706_hal::init();
    let heater = p.pins.p21.into_output(false);
    // This RAM-only example still requires board/ROM flash-pad and SPI-mode setup.
    // Confirm Probe::new's safety contract on the target before running it.
    // This program is only built, not executed on any device here.
    let probe = unsafe { Probe::new(p.flash, p.clocks) };
    bl706_hal::enable_interrupts();
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(check(log, heater, probe).unwrap());
    })
}
