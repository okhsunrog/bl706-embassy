//! Blocking UART0 TX and interrupt-driven async 8N1 UART0 on GPIO22/23.
use crate::{
    clock::Clocks,
    gpio::{Pin, Pull},
    interrupt, pac,
};
use core::{cell::RefCell, fmt, future::poll_fn, marker::PhantomData};
use critical_section::Mutex;
use embassy_time::Timer;

#[path = "uart_logic.rs"]
mod logic;
pub use logic::Error;
static STATE: Mutex<RefCell<logic::State>> = Mutex::new(RefCell::new(logic::State::new()));
const ALL: u32 = 0x1ff;
pub struct Uart0Token {
    _private: (),
}
impl Uart0Token {
    pub(crate) unsafe fn new() -> Self {
        Self { _private: () }
    }
}
pub struct Uart0 {
    _token: Uart0Token,
    _tx: Pin<22>,
}

impl Uart0 {
    pub fn new(token: Uart0Token, tx: Pin<22>, clocks: Clocks, baud: u32) -> Result<Self, Error> {
        let divider = logic::divider(clocks.uart_hz(), baud).ok_or(Error::InvalidBaud)?;
        tx.configure(7, Pull::None, true);
        pac::GLB.glb_parm().modify(|r| r.set_uart_swap_set(0));
        // Signal index = GPIO mod 8, UART0_TXD=2. Keep other signals intact.
        pac::GLB
            .UART_SIG_SEL_0()
            .modify(|r| r.0 = (r.0 & !(0xf << 24)) | (2 << 24));
        pac::UART0.utx_config().write(|_| {});
        pac::UART0.urx_config().write(|_| {});
        pac::UART0.uart_int_mask().write(|r| r.0 = ALL);
        pac::UART0.uart_int_en().write(|_| {});
        pac::UART0.uart_int_clear().write(|r| r.0 = 0x133);
        pac::UART0.uart_fifo_config_1().write(|_| {});
        pac::UART0.uart_sw_mode().write(|_| {});
        pac::UART0.data_config().write(|_| {});
        pac::UART0
            .uart_fifo_config_0()
            .write(|r| r.0 = (1 << 2) | (1 << 3));
        pac::UART0
            .uart_bit_prd()
            .write(|r| r.0 = u32::from(divider) * 0x0001_0001);
        // 8 data bits, one stop bit (encoded in half-bit units), free-run.
        pac::UART0.utx_config().write(|r| {
            r.set_cr_utx_bit_cnt_d(7);
            r.set_cr_utx_bit_cnt_p(1);
            r.set_cr_utx_frm_en(true);
            r.set_cr_utx_en(true);
        });
        Ok(Self {
            _token: token,
            _tx: tx,
        })
    }
    pub fn write(&mut self, bytes: &[u8]) -> Result<(), Error> {
        for &byte in bytes {
            let start = crate::time::now_ticks();
            while pac::UART0.uart_fifo_config_1().read().tx_fifo_cnt() == 0 {
                if crate::time::now_ticks().wrapping_sub(start) > 100_000 {
                    return Err(Error::Timeout);
                }
                core::hint::spin_loop();
            }
            pac::UART0
                .uart_fifo_wdata()
                .write(|r| r.set_uart_fifo_wdata(byte));
        }
        Ok(())
    }
}
impl fmt::Write for Uart0 {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.write(s.as_bytes()).map_err(|_| fmt::Error)
    }
}

impl Drop for Uart0 {
    fn drop(&mut self) {
        critical_section::with(|cs| {
            interrupt::disable(interrupt::UART0);
            let mut state = STATE.borrow(cs).borrow_mut();
            state.cancel(&mut Registers, true);
            state.cancel(&mut Registers, false);
            pac::UART0.uart_int_mask().write(|r| r.0 = ALL);
            pac::UART0.uart_int_en().write(|_| {});
            pac::UART0.utx_config().modify(|r| r.set_cr_utx_en(false));
            pac::UART0.urx_config().modify(|r| r.set_cr_urx_en(false));
            pac::UART0.uart_fifo_config_0().write(|r| r.0 = 12);
            crate::io_fence();
        });
    }
}

/// Async UART0, 8N1, no DMA or flow control. RX uses the hardware FIFO only.
/// Enable global interrupts before awaiting. See docs/uart.md for cancellation
/// and executor-latency limits. The blocking `Uart0` API remains available.
pub struct AsyncUart0 {
    _uart: Uart0,
    _rx: Pin<23>,
    character_us: u64,
}
/// Borrowed TX half; may run concurrently with the RX half.
pub struct UartTx<'a> {
    character_us: u64,
    _uart: PhantomData<&'a mut AsyncUart0>,
}
/// Borrowed RX half, backed only by the 128-byte hardware FIFO.
pub struct UartRx<'a> {
    _uart: PhantomData<&'a mut AsyncUart0>,
}
impl AsyncUart0 {
    pub fn new(
        token: Uart0Token,
        tx: Pin<22>,
        rx: Pin<23>,
        clocks: Clocks,
        baud: u32,
    ) -> Result<Self, Error> {
        let divider = logic::divider(clocks.uart_hz(), baud).ok_or(Error::InvalidBaud)?;
        let uart = Uart0::new(token, tx, clocks, baud)?;
        rx.configure(7, Pull::Up, true);
        // Signal 7 (GPIO23 % 8) selects UART0_RXD=3, signal 6 keeps TXD=2.
        pac::GLB
            .UART_SIG_SEL_0()
            .modify(|r| r.0 = (r.0 & !(0xf << 28)) | (3 << 28));
        pac::UART0.urx_config().write(|r| {
            r.set_cr_urx_bit_cnt_d(7);
            r.set_cr_urx_en(true);
        });
        pac::UART0
            .uart_int_en()
            .write(|r| r.0 = logic::TX_EVENTS | logic::RX_EVENTS);
        // SAFETY: token owns UART0, static state exists and all events are masked.
        unsafe { interrupt::enable(interrupt::UART0) };
        Ok(Self {
            _uart: uart,
            _rx: rx,
            character_us: (10_000_000 * (u64::from(divider) + 1))
                .div_ceil(u64::from(clocks.uart_hz())),
        })
    }
    pub fn split(&mut self) -> (UartTx<'_>, UartRx<'_>) {
        (
            UartTx {
                character_us: self.character_us,
                _uart: PhantomData,
            },
            UartRx { _uart: PhantomData },
        )
    }
    /// Receive at least one byte, or return zero for an empty buffer.
    pub async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Error> {
        read(bytes).await
    }
    /// Queue as many bytes as currently fit; use `Write::write_all` for all bytes.
    pub async fn write(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        write(bytes).await
    }
    /// Wait for the FIFO and final character on the wire to finish.
    pub async fn flush(&mut self) -> Result<(), Error> {
        flush(self.character_us).await
    }
}
impl UartTx<'_> {
    pub async fn write(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        write(bytes).await
    }
    pub async fn flush(&mut self) -> Result<(), Error> {
        flush(self.character_us).await
    }
}
impl UartRx<'_> {
    pub async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Error> {
        read(bytes).await
    }
}

struct Registers;
impl logic::Hardware for Registers {
    fn status(&mut self) -> u32 {
        pac::UART0.uart_int_sts().read().0
    }
    fn masked(&mut self) -> u32 {
        pac::UART0.uart_int_mask().read().0
    }
    fn mask(&mut self, events: u32) {
        pac::UART0.uart_int_mask().modify(|r| r.0 |= events);
        crate::io_fence();
    }
    fn unmask(&mut self, events: u32) {
        pac::UART0.uart_int_mask().modify(|r| r.0 &= !events);
        crate::io_fence();
    }
    fn tx_space(&mut self) -> u8 {
        pac::UART0.uart_fifo_config_1().read().tx_fifo_cnt()
    }
    fn rx_count(&mut self) -> u8 {
        pac::UART0.uart_fifo_config_1().read().rx_fifo_cnt()
    }
    fn tx_threshold(&mut self, value: u8) {
        pac::UART0
            .uart_fifo_config_1()
            .modify(|r| r.set_tx_fifo_th(value));
    }
    fn write(&mut self, byte: u8) {
        pac::UART0
            .uart_fifo_wdata()
            .write(|r| r.set_uart_fifo_wdata(byte));
    }
    fn read(&mut self) -> u8 {
        pac::UART0.uart_fifo_rdata().read().uart_fifo_rdata()
    }
    fn clear_tx(&mut self) {
        pac::UART0.uart_fifo_config_0().write(|r| r.0 = 1 << 2);
        crate::io_fence();
    }
    fn clear_rx(&mut self) {
        pac::UART0.uart_fifo_config_0().write(|r| r.0 = 1 << 3);
        pac::UART0.uart_int_clear().write(|r| r.0 = logic::PARITY);
        crate::io_fence();
    }
}

/// Cancels only this direction's IRQ subscription, preserving queued data and
/// the other half. The ISR never holds caller pointers, even if a future leaks.
struct WaitGuard {
    receive: bool,
}
impl Drop for WaitGuard {
    fn drop(&mut self) {
        critical_section::with(|cs| {
            STATE
                .borrow(cs)
                .borrow_mut()
                .cancel(&mut Registers, self.receive);
        });
    }
}
async fn read(bytes: &mut [u8]) -> Result<usize, Error> {
    let _guard = WaitGuard { receive: true };
    poll_fn(|cx| {
        critical_section::with(|cs| {
            STATE
                .borrow(cs)
                .borrow_mut()
                .poll_read(&mut Registers, cx, bytes)
        })
    })
    .await
}
async fn write(bytes: &[u8]) -> Result<usize, Error> {
    let _guard = WaitGuard { receive: false };
    poll_fn(|cx| {
        critical_section::with(|cs| {
            STATE
                .borrow(cs)
                .borrow_mut()
                .poll_write(&mut Registers, cx, bytes)
        })
    })
    .await
}
async fn flush(character_us: u64) -> Result<(), Error> {
    let _guard = WaitGuard { receive: false };
    poll_fn(|cx| {
        critical_section::with(|cs| STATE.borrow(cs).borrow_mut().poll_drain(&mut Registers, cx))
    })
    .await?;
    // Free-run disables TX_END. Once FIFO empty IRQ fires,
    // wait cooperatively for the shift register, at most one 8N1 character at
    // a time. Check BUS_BUSY as well: FIFO empty alone is not a flush.
    while pac::UART0.uart_status().read().sts_utx_bus_busy() {
        Timer::after_micros(character_us).await;
    }
    Ok(())
}
pub(crate) fn on_interrupt() {
    let wakers =
        critical_section::with(|cs| STATE.borrow(cs).borrow_mut().on_interrupt(&mut Registers));
    for waker in wakers.into_iter().flatten() {
        waker.wake();
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidBaud => "invalid UART baud rate",
            Self::Timeout => "UART timeout",
            Self::Fifo => "UART TX FIFO error",
            Self::Overrun => "UART RX FIFO error",
            Self::Parity => "UART parity error",
        })
    }
}
impl core::error::Error for Error {}
impl embedded_io::Error for Error {
    fn kind(&self) -> embedded_io::ErrorKind {
        match self {
            Self::InvalidBaud => embedded_io::ErrorKind::InvalidInput,
            Self::Timeout => embedded_io::ErrorKind::TimedOut,
            Self::Parity => embedded_io::ErrorKind::InvalidData,
            Self::Fifo | Self::Overrun => embedded_io::ErrorKind::Other,
        }
    }
}
macro_rules! error_type {
    ($($ty:ty),*) => { $(impl embedded_io::ErrorType for $ty { type Error = Error; })* };
}
error_type!(AsyncUart0, UartTx<'_>, UartRx<'_>);
impl embedded_io_async::Read for AsyncUart0 {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Error> {
        self.read(bytes).await
    }
}
impl embedded_io_async::Write for AsyncUart0 {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        self.write(bytes).await
    }
    async fn flush(&mut self) -> Result<(), Error> {
        self.flush().await
    }
}
impl embedded_io_async::Read for UartRx<'_> {
    async fn read(&mut self, bytes: &mut [u8]) -> Result<usize, Error> {
        self.read(bytes).await
    }
}
impl embedded_io_async::Write for UartTx<'_> {
    async fn write(&mut self, bytes: &[u8]) -> Result<usize, Error> {
        self.write(bytes).await
    }
    async fn flush(&mut self) -> Result<(), Error> {
        self.flush().await
    }
}
