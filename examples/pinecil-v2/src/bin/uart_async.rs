//! Compile-only until an external 3.3 V UART is connected; see docs/uart.md.
#![no_std]
#![no_main]
use bl706_hal::{
    embedded_io_async::Write,
    gpio::Output,
    uart::{AsyncUart0, UartRx, UartTx},
};
use core::fmt::Write as _;
use embassy_time::{Duration, Timer, with_timeout};
use pinecil_v2_bringup::{Logger, banner};
use static_cell::StaticCell;

static EXECUTOR: StaticCell<bl706_hal::Executor> = StaticCell::new();
static UART: StaticCell<AsyncUart0> = StaticCell::new();

#[embassy_executor::task]
async fn transmit(mut tx: UartTx<'static>) {
    // Exceeds the hardware FIFO: write_all must wait for IRQ-driven progress.
    let mut data = [b'U'; 320];
    data[318..].copy_from_slice(b"\r\n");
    loop {
        let result = with_timeout(Duration::from_secs(1), async {
            tx.write_all(&data).await?;
            tx.flush().await
        })
        .await;
        rtt_target::rprintln!("UART TX: {:?}", result);
        Timer::after_secs(2).await;
    }
}
#[embassy_executor::task]
async fn receive(mut rx: UartRx<'static>) {
    let mut data = [0; 32];
    loop {
        // Repeated idle cancellation/rearm; a partial read is a valid result.
        match with_timeout(Duration::from_secs(1), rx.read(&mut data)).await {
            Ok(Ok(n)) => {
                rtt_target::rprintln!("UART RX {}: {:02x?}", n, &data[..n]);
            }
            result => {
                rtt_target::rprintln!("UART RX: {:?}", result);
            }
        }
    }
}
#[embassy_executor::task]
async fn heartbeat(mut log: Logger, _heater_off: Output<21>) {
    banner(&mut log, "async UART0 GPIO22 TX / GPIO23 RX, 115200 8N1");
    loop {
        let _ = writeln!(
            log,
            "UART heartbeat {}\r",
            embassy_time::Instant::now().as_secs()
        );
        Timer::after_secs(1).await;
    }
}
#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let log = Logger::init();
    let p = bl706_hal::init();
    let heater_off = p.pins.p21.into_output(false);
    let uart =
        UART.init(AsyncUart0::new(p.uart0, p.pins.p22, p.pins.p23, p.clocks, 115_200).unwrap());
    let (tx, rx) = uart.split();
    bl706_hal::enable_interrupts();
    EXECUTOR.init(bl706_hal::Executor::new()).run(|s| {
        s.spawn(transmit(tx).unwrap());
        s.spawn(receive(rx).unwrap());
        s.spawn(heartbeat(log, heater_off).unwrap());
    })
}
