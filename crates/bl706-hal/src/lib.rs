//! Experimental BL702/704/706 bring-up HAL; first BL706 RAM heartbeat validated.
//! See docs/validation.md for the scope and remaining hardware checks.
#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

pub use bl706_pac as pac;
pub use embedded_hal_async;
pub use embedded_io_async;
pub use executor::{Executor, InterruptExecutor};
/// Software fallback atomics, unified with static_cell by Cargo features.
pub use portable_atomic;
pub mod adc;
pub mod clock;
mod clock_logic;
pub mod executor;
pub mod flash;
mod flash_logic;
pub mod gpio;
pub mod i2c;
pub mod interrupt;
mod peripheral_logic;
pub mod pwm;
pub mod radio;
pub mod reset;
mod runtime;
pub mod time;
pub mod timer;
pub mod uart;
pub mod watchdog;

use core::cell::Cell;
use critical_section::Mutex;
static TAKEN: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));

pub struct Peripherals {
    pub radio: radio::RadioToken,
    pub software_interrupt: executor::InterruptToken,
    pub pins: gpio::Pins,
    pub uart0: uart::Uart0Token,
    pub i2c: i2c::I2cToken,
    pub adc: adc::AdcToken,
    pub timer0: timer::TimerToken<0>,
    pub timer1: timer::TimerToken<1>,
    pub pwm0: pwm::PwmToken<0>,
    pub pwm1: pwm::PwmToken<1>,
    pub pwm2: pwm::PwmToken<2>,
    pub pwm3: pwm::PwmToken<3>,
    pub pwm4: pwm::PwmToken<4>,
    pub clocks: clock::Clocks,
    pub watchdog: watchdog::WatchdogToken,
    pub reset_reason: reset::ResetReason,
    pub flash: flash::FlashToken,
}

/// Initialize once in machine mode, after this runtime's RAM reset entry.
pub fn init() -> Peripherals {
    init_with_config(clock::Config::default()).expect("default clock initialization failed")
}

/// Freeze clocks before constructing any peripheral drivers. Retry is allowed after XTAL timeout.
pub fn init_with_config(config: clock::Config) -> Result<Peripherals, clock::Error> {
    config.plan()?;
    critical_section::with(|cs| {
        if TAKEN.borrow(cs).get() {
            return Err(clock::Error::AlreadyInitialized);
        }
        // SAFETY: sole initialization. Global IRQ enable is left to the caller
        // after the critical section, so no interrupt can observe partial state.
        unsafe {
            let reset_reason = reset::reason();
            let clocks = clock::init(config)?;
            TAKEN.borrow(cs).set(true);
            interrupt::init();
            gpio::init();
            time::init(clocks);
            Ok(Peripherals {
                radio: radio::RadioToken::new(),
                software_interrupt: executor::InterruptToken::new(),
                pins: gpio::Pins::new(),
                uart0: uart::Uart0Token::new(),
                i2c: i2c::I2cToken::new(),
                adc: adc::AdcToken::new(),
                timer0: timer::TimerToken::new(),
                timer1: timer::TimerToken::new(),
                pwm0: pwm::PwmToken::new(),
                pwm1: pwm::PwmToken::new(),
                pwm2: pwm::PwmToken::new(),
                pwm3: pwm::PwmToken::new(),
                pwm4: pwm::PwmToken::new(),
                clocks,
                watchdog: watchdog::WatchdogToken::new(),
                reset_reason,
                flash: flash::FlashToken::new(),
            })
        }
    })
}

pub(crate) fn io_fence() {
    unsafe {
        core::arch::asm!("fence iorw, iorw");
    }
}

/// Enable machine interrupts after initialization. All installed handlers must
/// be ready; the runtime starts with MIE cleared.
pub fn enable_interrupts() {
    unsafe { interrupt::enable_global() };
}

struct SingleHart;
critical_section::set_impl!(SingleHart);
// SAFETY: BL706 is single-hart. NMI/debug/DMA must not access protected state.
// Nested sections restore the previous MIE state; no LR/SC or AMO is involved.
unsafe impl critical_section::Impl for SingleHart {
    unsafe fn acquire() -> bool {
        let old: usize;
        unsafe { core::arch::asm!("csrrci {}, mstatus, 8", out(reg) old) };
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        old & 8 != 0
    }
    unsafe fn release(restore: bool) {
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        if restore {
            unsafe { core::arch::asm!("csrsi mstatus, 8") };
        }
    }
}
