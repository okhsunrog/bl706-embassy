//! BL70x's older SiFive CLIC byte arrays, not PLIC or T-Head CLIC.
use core::ptr::{read_volatile, write_volatile};
pub const MTIME: u8 = 7;
pub const I2C: u8 = 48;
const BASE: usize = 0x0280_0000;

pub(crate) unsafe fn init() {
    unsafe {
        for n in 0..80 {
            write_volatile((BASE + 0x400 + n) as *mut u8, 0);
            write_volatile((BASE + n) as *mut u8, 0);
            write_volatile((BASE + 0x800 + n) as *mut u8, 0);
        }
        write_volatile((BASE + 0xc00) as *mut u8, 0);
    }
}

/// Enable an initialized handler. This runtime installs MTIME and I2C only.
/// # Safety
/// The caller must own the peripheral and initialize all handler state.
pub unsafe fn enable(n: u8) {
    assert!(n < 80);
    unsafe {
        core::arch::asm!("fence iorw, iorw");
        write_volatile((BASE + 0x400 + n as usize) as *mut u8, 1);
    }
}
pub(crate) fn disable(n: u8) {
    assert!(n < 80);
    unsafe {
        write_volatile((BASE + 0x400 + n as usize) as *mut u8, 0);
        core::arch::asm!("fence iorw, iorw");
    }
}
pub fn is_enabled(n: u8) -> bool {
    assert!(n < 80);
    unsafe { read_volatile((BASE + 0x400 + n as usize) as *const u8) != 0 }
}
pub(crate) unsafe fn enable_global() {
    unsafe { core::arch::asm!("csrsi mstatus, 8") };
}
#[unsafe(no_mangle)]
extern "C" fn bl706_irq_dispatch(number: usize) {
    match number {
        7 => crate::time::on_interrupt(),
        48 => crate::i2c::on_interrupt(),
        _ => unreachable!(),
    }
}
