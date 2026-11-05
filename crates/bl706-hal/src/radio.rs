//! IRQ bridge for an external BL70x radio controller. No radio configuration.

use core::cell::Cell;
use critical_section::Mutex;

pub type Handler = unsafe extern "C" fn();
static RADIO: Mutex<Cell<Option<Handler>>> = Mutex::new(Cell::new(None));
static BOOT_SOFTWARE: Mutex<Cell<Option<Handler>>> = Mutex::new(Cell::new(None));

pub struct RadioToken {
    _private: (),
}
impl RadioToken {
    pub(crate) const fn new() -> Self {
        Self { _private: () }
    }
}

/// Install a radio callback before enabling IRQ72.
///
/// # Safety
/// The caller owns `RadioToken`, keeps the callback state alive, and serializes
/// changes against the radio interrupt. The callback must acknowledge its source.
pub unsafe fn set_handler(handler: Option<Handler>) {
    critical_section::with(|cs| RADIO.borrow(cs).set(handler));
}

/// Temporarily redirect software IRQ3 for early RF calibration.
///
/// # Safety
/// The caller exclusively borrows the unused software-interrupt token, before
/// starting an InterruptExecutor, and restores None before releasing that borrow.
pub unsafe fn set_boot_software_handler(handler: Option<Handler>) {
    critical_section::with(|cs| BOOT_SOFTWARE.borrow(cs).set(handler));
}

pub fn handler() -> Option<Handler> {
    critical_section::with(|cs| RADIO.borrow(cs).get())
}
pub fn boot_software_handler() -> Option<Handler> {
    critical_section::with(|cs| BOOT_SOFTWARE.borrow(cs).get())
}

pub(crate) fn on_interrupt() {
    if let Some(handler) = handler() {
        // SAFETY: registration requires a live, initialized callback.
        unsafe { handler() };
    }
}
pub(crate) fn on_boot_software_interrupt() -> bool {
    if let Some(handler) = boot_software_handler() {
        crate::interrupt::clear_software();
        unsafe {
            core::ptr::write_volatile(0x0280_0003 as *mut u8, 0);
            handler();
        }
        true
    } else {
        false
    }
}
