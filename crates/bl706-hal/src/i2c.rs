//! IRQ-driven, cancellation-safe BL70x I2C master for register devices/FUSB302.
//! Supports one packet (1..=256 bytes), or write(1..=4)+read using sub-address
//! hardware for repeated START. Unsupported transaction shapes fail before I/O.
use crate::{
    clock::Clocks,
    gpio::{Pin, Pull},
    interrupt, pac,
};
use core::{future::poll_fn, task::Poll};
use embassy_sync::waitqueue::AtomicWaker;
use embassy_time::{Duration, with_timeout};
use embedded_hal::i2c::{ErrorKind, ErrorType, NoAcknowledgeSource, Operation};

#[path = "i2c_logic.rs"]
mod logic;
static WAKER: AtomicWaker = AtomicWaker::new();
const ALL: u32 = 0x3f;
const CLEAR: u32 = (1 << 16) | (1 << 19) | (1 << 20);
const ENABLE: u32 = ALL << 24;
const ERRORS: u32 = (1 << 3) | (1 << 4) | (1 << 5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidAddress,
    Unsupported,
    BusBusy,
    NoAcknowledge,
    ArbitrationLost,
    Fifo,
    ShortTransfer,
    Timeout,
}
impl embedded_hal::i2c::Error for Error {
    fn kind(&self) -> ErrorKind {
        match self {
            Self::NoAcknowledge => ErrorKind::NoAcknowledge(NoAcknowledgeSource::Unknown),
            Self::ArbitrationLost => ErrorKind::ArbitrationLoss,
            Self::Fifo => ErrorKind::Overrun,
            _ => ErrorKind::Other,
        }
    }
}
pub struct I2cToken {
    _private: (),
}
impl I2cToken {
    pub(crate) unsafe fn new() -> Self {
        Self { _private: () }
    }
}
pub struct I2c {
    _token: I2cToken,
    _scl: Pin<10>,
    _sda: Pin<11>,
    timeout: Duration,
}
enum Buffer<'a> {
    Read(&'a mut [u8]),
    Write(&'a [u8]),
}
impl Buffer<'_> {
    fn len(&self) -> usize {
        match self {
            Self::Read(b) => b.len(),
            Self::Write(b) => b.len(),
        }
    }
    fn is_read(&self) -> bool {
        matches!(self, Self::Read(_))
    }
}

impl I2c {
    /// GPIO10=SCL and GPIO11=SDA per RM/SDK alternate function 6. IronOS's
    /// Pins.h labels these in reverse; see docs/hardware.md. External pull-ups.
    /// Initial bring-up uses nominal 100 kHz, with SCL sync (clock stretching).
    pub fn new(token: I2cToken, scl: Pin<10>, sda: Pin<11>, _clocks: Clocks) -> Self {
        scl.configure(6, Pull::None, true);
        sda.configure(6, Pull::None, true);
        abort();
        // SDK's simple equal-phase model. 32MHz/(4*(79+1)) = 100kHz before
        // synchronizer delay. Validate actual rate and tLOW/tHIGH on hardware.
        let period = 79 * 0x0101_0101;
        pac::I2C.i2c_prd_start().write(|r| r.0 = period);
        pac::I2C.i2c_prd_stop().write(|r| r.0 = period);
        pac::I2C.i2c_prd_data().write(|r| r.0 = period);
        pac::I2C.i2c_fifo_config_1().write(|_| {}); // FIFO thresholds = zero
        Self {
            _token: token,
            _scl: scl,
            _sda: sda,
            timeout: Duration::from_millis(100),
        }
    }

    pub fn set_timeout(&mut self, timeout: Duration) {
        self.timeout = timeout;
    }

    async fn packet(
        &mut self,
        address: u8,
        sub: &[u8],
        mut buffer: Buffer<'_>,
    ) -> Result<(), Error> {
        let len = buffer.len();
        if address > 0x7f {
            return Err(Error::InvalidAddress);
        }
        if !(1..=256).contains(&len) || sub.len() > 4 {
            return Err(Error::Unsupported);
        }
        if pac::I2C.i2c_bus_busy().read().sts_i2c_bus_busy() {
            return Err(Error::BusBusy);
        }
        abort();
        let guard = TransferGuard;
        pac::I2C.i2c_sub_addr().write(|r| r.0 = logic::pack(sub));
        pac::I2C.i2c_config().write(|r| {
            r.set_cr_i2c_pkt_dir(buffer.is_read());
            r.set_cr_i2c_scl_sync_en(true);
            r.set_cr_i2c_sub_addr_en(!sub.is_empty());
            r.set_cr_i2c_sub_addr_bc(sub.len().saturating_sub(1) as u8);
            r.set_cr_i2c_slv_addr(address);
            r.set_cr_i2c_pkt_len((len - 1) as u8);
        });
        let mut done = 0;
        if let Buffer::Write(bytes) = &buffer {
            push(bytes, &mut done);
        }
        pac::I2C.i2c_config().modify(|r| r.set_cr_i2c_m_en(true));
        let result = with_timeout(
            self.timeout,
            poll_fn(|cx| {
                critical_section::with(|_| {
                    // Register first, then inspect status, then unmask. A level IRQ
                    // arriving between these operations remains observable.
                    WAKER.register(cx.waker());
                    let status = pac::I2C.i2c_int_sts().read().0;
                    if status & (1 << 3) != 0 {
                        return Poll::Ready(Err(Error::NoAcknowledge));
                    }
                    if status & (1 << 4) != 0 {
                        return Poll::Ready(Err(Error::ArbitrationLost));
                    }
                    if status & (1 << 5) != 0 {
                        return Poll::Ready(Err(Error::Fifo));
                    }
                    match &mut buffer {
                        Buffer::Read(bytes) => pull(bytes, &mut done),
                        Buffer::Write(bytes) => push(bytes, &mut done),
                    }
                    if status & 1 != 0 {
                        return Poll::Ready(if done == len {
                            Ok(())
                        } else {
                            Err(Error::ShortTransfer)
                        });
                    }
                    let fifo = if done == len {
                        0
                    } else if buffer.is_read() {
                        1 << 2
                    } else {
                        1 << 1
                    };
                    // Explicit write: status and W1C fields share this register.
                    pac::I2C
                        .i2c_int_sts()
                        .write(|r| r.0 = ENABLE | ((ALL & !(1 | ERRORS | fifo)) << 8));
                    unsafe { interrupt::enable(interrupt::I2C) };
                    Poll::Pending
                })
            }),
        )
        .await;
        drop(guard); // Same cleanup on success, error, timeout and future drop.
        result.unwrap_or(Err(Error::Timeout))
    }
}

fn push(bytes: &[u8], done: &mut usize) {
    // Count is available FIFO words, not bytes (depth = two words).
    logic::push(
        bytes,
        done,
        || pac::I2C.i2c_fifo_config_1().read().tx_fifo_cnt(),
        |word| pac::I2C.i2c_fifo_wdata().write(|r| r.0 = word),
    );
}
fn pull(bytes: &mut [u8], done: &mut usize) {
    logic::pull(
        bytes,
        done,
        || pac::I2C.i2c_fifo_config_1().read().rx_fifo_cnt(),
        || pac::I2C.i2c_fifo_rdata().read().0,
    );
}
fn abort() {
    interrupt::disable(interrupt::I2C);
    pac::I2C.i2c_config().modify(|r| r.set_cr_i2c_m_en(false));
    pac::I2C.i2c_int_sts().write(|r| r.0 = (ALL << 8) | CLEAR);
    pac::I2C
        .i2c_fifo_config_0()
        .write(|r| r.0 = (1 << 2) | (1 << 3));
}
struct TransferGuard;
impl Drop for TransferGuard {
    fn drop(&mut self) {
        abort();
    }
}
impl Drop for I2c {
    fn drop(&mut self) {
        abort();
    }
}
pub(crate) fn on_interrupt() {
    // Buffer pointers are never shared with the ISR. The task services FIFO.
    interrupt::disable(interrupt::I2C);
    WAKER.wake();
}
impl ErrorType for I2c {
    type Error = Error;
}
impl embedded_hal_async::i2c::I2c for I2c {
    async fn transaction(
        &mut self,
        address: u8,
        operations: &mut [Operation<'_>],
    ) -> Result<(), Error> {
        // Validate the entire shape before starting any transfer. Never insert
        // STOP between operations that promise repeated START to the caller.
        match operations {
            [] => Ok(()),
            [Operation::Read(b)] => self.packet(address, &[], Buffer::Read(b)).await,
            [Operation::Write(b)] => self.packet(address, &[], Buffer::Write(b)).await,
            [Operation::Write(sub), Operation::Read(b)] if (1..=4).contains(&sub.len()) => {
                self.packet(address, sub, Buffer::Read(b)).await
            }
            _ => Err(Error::Unsupported),
        }
    }
}
