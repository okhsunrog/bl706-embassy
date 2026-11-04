//! Experimental BL702/704/706 bring-up HAL. No hardware validation yet.
#![no_std]
#![deny(unsafe_op_in_unsafe_fn)]

pub use bl706_pac as pac;
pub use embassy_executor::Executor;
pub use embedded_hal_async;
/// Software fallback atomics, unified with static_cell by Cargo features.
pub use portable_atomic;
pub mod adc;
pub mod clock;
pub mod gpio;
pub mod i2c;
pub mod interrupt;
mod peripheral_logic;
pub mod pwm;
mod runtime;
pub mod time;
pub mod timer;
pub mod uart;

use core::cell::Cell;
use critical_section::Mutex;
static TAKEN: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));

pub struct Peripherals {
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
}

/// Initialize once in machine mode, after this runtime's RAM reset entry.
pub fn init() -> Peripherals {
    critical_section::with(|cs| {
        assert!(!TAKEN.borrow(cs).replace(true), "HAL initialized twice");
        // SAFETY: sole initialization. Global IRQ enable is left to the caller
        // after the critical section, so no interrupt can observe partial state.
        unsafe {
            interrupt::init();
            gpio::init();
            let clocks = clock::init();
            time::init();
            Peripherals {
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
            }
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
