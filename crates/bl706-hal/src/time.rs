//! Embassy 1 MHz time driver using the 64-bit core-local mtime/mtimecmp.
use crate::{interrupt, pac};
use core::{
    cell::RefCell,
    ptr::{read_volatile, write_volatile},
    task::Waker,
};
use critical_section::Mutex;
use embassy_time_driver::Driver;
use embassy_time_queue_utils::Queue;
const MTIME: usize = 0x0200_bff8;
const CMP: usize = 0x0200_4000;

struct TimeDriver {
    queue: Mutex<RefCell<Queue>>,
}
embassy_time_driver::time_driver_impl!(static DRIVER: TimeDriver = TimeDriver { queue: Mutex::new(RefCell::new(Queue::new())) });

pub fn now_ticks() -> u64 {
    loop {
        let high = unsafe { read_volatile((MTIME + 4) as *const u32) };
        let low = unsafe { read_volatile(MTIME as *const u32) };
        let high2 = unsafe { read_volatile((MTIME + 4) as *const u32) };
        if high == high2 {
            return (u64::from(high) << 32) | u64::from(low);
        }
    }
}
fn set_compare(at: u64) {
    // RV32 safe sequence: prevent a transient small comparison value.
    unsafe {
        write_volatile(CMP as *mut u32, u32::MAX);
        write_volatile((CMP + 4) as *mut u32, (at >> 32) as u32);
        write_volatile(CMP as *mut u32, at as u32);
        core::arch::asm!("fence iorw, iorw");
    }
}
impl Driver for TimeDriver {
    fn now(&self) -> u64 {
        now_ticks()
    }
    fn schedule_wake(&self, at: u64, waker: &Waker) {
        critical_section::with(|cs| {
            let mut queue = self.queue.borrow(cs).borrow_mut();
            queue.schedule_wake(at, waker);
            arm(&mut queue);
        });
    }
}
fn arm(queue: &mut Queue) {
    // next_expiration also wakes expired tasks. Recheck time after programming
    // the comparator to close the deadline-passed-during-programming race.
    loop {
        let next = queue.next_expiration(now_ticks());
        set_compare(next);
        if next == u64::MAX || now_ticks() < next {
            break;
        }
    }
}
pub(crate) fn on_interrupt() {
    critical_section::with(|cs| arm(&mut DRIVER.queue.borrow(cs).borrow_mut()));
}
pub(crate) unsafe fn init() {
    pac::GLB.CPU_CLK_CFG().modify(|r| r.set_cpu_rtc_en(false));
    pac::GLB.CPU_CLK_CFG().modify(|r| {
        r.set_cpu_rtc_sel(false); // BCLK
        r.set_cpu_rtc_div(31); // 32 MHz / (31+1) = nominal 1 MHz
    });
    set_compare(u64::MAX);
    // Start at zero for reproducible deadlines; no existing driver may be alive.
    unsafe {
        write_volatile(MTIME as *mut u32, 0);
        write_volatile((MTIME + 4) as *mut u32, 0);
    }
    pac::GLB.CPU_CLK_CFG().modify(|r| r.set_cpu_rtc_en(true));
    unsafe { interrupt::enable(interrupt::MTIME) };
}
