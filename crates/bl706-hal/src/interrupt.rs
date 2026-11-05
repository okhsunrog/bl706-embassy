//! BL70x's older SiFive CLIC byte arrays, not PLIC or T-Head CLIC.
use core::ptr::{read_volatile, write_volatile};
pub const MSOFT: u8 = 3;
pub const MTIME: u8 = 7;
pub const UART0: u8 = 45;
pub const I2C: u8 = 48;
pub const GPIO: u8 = 60;
// SDK peripheral numbers are relative to IRQ_NUM_BASE=16 on RISC-V.
pub const ADC: u8 = 16 + 25;
pub const PWM: u8 = 16 + 34;
pub const TIMER0: u8 = 52;
pub const TIMER1: u8 = 53;
const BASE: usize = 0x0280_0000;
// BL702 SDK e24/clic.h: CLINT base 0x02000000, MSIP offset 0, 32-bit.
const MSIP: *mut u32 = 0x0200_0000 as *mut u32;

pub(crate) fn pend_software() {
    unsafe { write_volatile(MSIP, 1) };
    crate::io_fence();
}
pub(crate) fn clear_software() {
    unsafe { write_volatile(MSIP, 0) };
    crate::io_fence();
}

pub(crate) unsafe fn init() {
    clear_software();
    unsafe {
        for n in 0..80 {
            write_volatile((BASE + 0x400 + n) as *mut u8, 0);
            write_volatile((BASE + n) as *mut u8, 0);
            // One level bit: peripherals at level 255; software IRQ at 127.
            // Low unimplemented CFG bits read as ones on this older CLIC.
            write_volatile((BASE + 0x800 + n) as *mut u8, 0x80);
        }
        write_volatile((BASE + 0x800 + usize::from(MSOFT)) as *mut u8, 0);
        // E24 manual 8.7.8: nlBits=1 in bits[4:1], nvBits=nmBits=0.
        write_volatile((BASE + 0xc00) as *mut u8, 2);
        crate::io_fence();
    }
}

/// Enable a source supported by this runtime after initializing its handler state.
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
        3 => crate::executor::on_interrupt(),
        7 => crate::time::on_interrupt(),
        41 => crate::adc::on_interrupt(),
        45 => crate::uart::on_interrupt(),
        48 => crate::i2c::on_interrupt(),
        50 => crate::pwm::on_interrupt(),
        52 => crate::timer::on_interrupt(0),
        53 => crate::timer::on_interrupt(1),
        60 => crate::gpio::on_interrupt(),
        _ => unreachable!(),
    }
}
