//! IRQ-driven, cancellation-safe BL70x I2C master for register devices/FUSB302.
//! Supports one packet (1..=256 bytes), or write(1..=4)+read using sub-address
//! hardware for repeated START. Unsupported transaction shapes fail before I/O.
use crate::{
    clock::Clocks,
    gpio::{Pin, Pull},
    interrupt, pac,
};
use core::{cell::RefCell, future::poll_fn, task::Poll};
use critical_section::Mutex;
use embassy_sync::waitqueue::AtomicWaker;
use embassy_time::{Duration, with_timeout};
use embedded_hal::i2c::{ErrorKind, ErrorType, NoAcknowledgeSource, Operation};

#[path = "i2c_logic.rs"]
mod logic;
static WAKER: AtomicWaker = AtomicWaker::new();
static TRANSFER: Mutex<RefCell<logic::Transfer>> = Mutex::new(RefCell::new(logic::Transfer::new()));
const ALL: u32 = 0x3f;
const CLEAR: u32 = (1 << 16) | (1 << 19) | (1 << 20);
const ENABLE: u32 = ALL << 24;

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
impl From<logic::TransferError> for Error {
    fn from(error: logic::TransferError) -> Self {
        match error {
            logic::TransferError::NoAcknowledge => Self::NoAcknowledge,
            logic::TransferError::ArbitrationLost => Self::ArbitrationLost,
            logic::TransferError::Fifo => Self::Fifo,
            logic::TransferError::ShortTransfer => Self::ShortTransfer,
        }
    }
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
    pub fn new(token: I2cToken, scl: Pin<10>, sda: Pin<11>, clocks: Clocks) -> Self {
        scl.configure(6, Pull::None, true);
        sda.configure(6, Pull::None, true);
        abort();
        // SDK equal-phase model, rounded conservatively to <=100kHz before sync delay.
        let period = u32::from(clocks.i2c_phase()) * 0x0101_0101;
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

    /// Change the nominal SCL rate while the owned bus is idle. SCL sync can
    /// lower the effective rate. Standard/fast mode rates up to 400 kHz only.
    pub fn set_frequency(&mut self, clocks: Clocks, hz: u32) -> Result<(), Error> {
        let phase = logic::clock_phase(clocks.bus_hz(), hz).ok_or(Error::Unsupported)?;
        if critical_section::with(|cs| TRANSFER.borrow(cs).borrow().is_active())
            || pac::I2C.i2c_bus_busy().read().sts_i2c_bus_busy()
        {
            return Err(Error::BusBusy);
        }
        let period = u32::from(phase) * 0x0101_0101;
        pac::I2C.i2c_prd_start().write(|r| r.0 = period);
        pac::I2C.i2c_prd_stop().write(|r| r.0 = period);
        pac::I2C.i2c_prd_data().write(|r| r.0 = period);
        Ok(())
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
        if !(1..=logic::MAX_PACKET).contains(&len) || sub.len() > 4 {
            return Err(Error::Unsupported);
        }
        let guard = critical_section::with(|cs| {
            let mut transfer = TRANSFER.borrow(cs).borrow_mut();
            // A forgotten future can leave an owned packet running. Never
            // overwrite it while the ISR is still servicing that packet.
            if transfer.is_active() || pac::I2C.i2c_bus_busy().read().sts_i2c_bus_busy() {
                return Err(Error::BusBusy);
            }
            let mut hw = RegisterHardware;
            transfer.cancel(&mut hw);
            pac::I2C.i2c_sub_addr().write(|r| r.0 = logic::pack(sub));
            pac::I2C.i2c_config().write(|r| {
                r.set_cr_i2c_pkt_dir(buffer.is_read());
                r.set_cr_i2c_scl_sync_en(true);
                // Same one-cycle deglitch filter used by Pinecil V2 IronOS.
                r.set_cr_i2c_deg_en(true);
                r.set_cr_i2c_deg_cnt(1);
                r.set_cr_i2c_sub_addr_en(!sub.is_empty());
                r.set_cr_i2c_sub_addr_bc(sub.len().saturating_sub(1) as u8);
                r.set_cr_i2c_slv_addr(address);
                r.set_cr_i2c_pkt_len((len - 1) as u8);
            });
            match &buffer {
                Buffer::Read(_) => transfer.begin_read(len),
                Buffer::Write(bytes) => transfer.begin_write(bytes),
            }
            // Only the pre-start TX preload runs in the task. All subsequent
            // FIFO progress and terminal/error latching belong to the ISR.
            transfer.prime(&mut hw);
            unsafe { interrupt::enable(interrupt::I2C) };
            pac::I2C.i2c_config().modify(|r| r.set_cr_i2c_m_en(true));
            unsafe { core::arch::asm!("fence iorw, iorw") };
            Ok(TransferGuard)
        })?;
        let result = with_timeout(
            self.timeout,
            poll_fn(|cx| {
                critical_section::with(|cs| {
                    // Completion may precede the first poll. Register first,
                    // then observe the latched result; the IRQ never waits for
                    // executor progress and never uses the caller's buffer.
                    WAKER.register(cx.waker());
                    let transfer = TRANSFER.borrow(cs).borrow();
                    match transfer.result() {
                        Some(Ok(())) => {
                            if let Buffer::Read(out) = &mut buffer {
                                transfer.copy_read(out);
                            }
                            Poll::Ready(Ok(()))
                        }
                        Some(Err(error)) => Poll::Ready(Err(error.into())),
                        None => Poll::Pending,
                    }
                })
            }),
        )
        .await;
        drop(guard); // Same cleanup on success, error, timeout and future drop.
        result.unwrap_or(Err(Error::Timeout))
    }
}

/// MMIO is private to the owned transfer engine and its initialization path.
struct RegisterHardware;
impl logic::Hardware for RegisterHardware {
    fn status(&mut self) -> u32 {
        pac::I2C.i2c_int_sts().read().0
    }
    fn tx_space(&mut self) -> u8 {
        pac::I2C.i2c_fifo_config_1().read().tx_fifo_cnt()
    }
    fn rx_words(&mut self) -> u8 {
        pac::I2C.i2c_fifo_config_1().read().rx_fifo_cnt()
    }
    fn write_word(&mut self, word: u32) {
        pac::I2C.i2c_fifo_wdata().write(|r| r.0 = word);
    }
    fn read_word(&mut self) -> u32 {
        pac::I2C.i2c_fifo_rdata().read().0
    }
    fn listen(&mut self, events: u32) {
        // Explicit write: status, masks, enables and W1C share this register.
        // No clear bits during service/rearming: an END/error may race this write.
        pac::I2C
            .i2c_int_sts()
            .write(|r| r.0 = ENABLE | ((ALL & !events) << 8));
    }
    fn stop(&mut self) {
        interrupt::disable(interrupt::I2C);
        pac::I2C.i2c_config().modify(|r| r.set_cr_i2c_m_en(false));
        pac::I2C.i2c_int_sts().write(|r| r.0 = (ALL << 8) | CLEAR);
        pac::I2C
            .i2c_fifo_config_0()
            .write(|r| r.0 = (1 << 2) | (1 << 3));
        unsafe { core::arch::asm!("fence iorw, iorw") };
    }
}
fn abort() {
    critical_section::with(|cs| {
        TRANSFER
            .borrow(cs)
            .borrow_mut()
            .cancel(&mut RegisterHardware)
    });
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
    let completed = critical_section::with(|cs| {
        TRANSFER
            .borrow(cs)
            .borrow_mut()
            .on_interrupt(&mut RegisterHardware)
    });
    // Wake only on completion/error, after dropping the state borrow. FIFO
    // interrupts leave the CPU line enabled and are serviced entirely here.
    if completed {
        WAKER.wake();
    }
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
