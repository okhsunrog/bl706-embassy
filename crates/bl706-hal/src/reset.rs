//! Reset flags are independent/sticky, not a mutually exclusive cause enum.
use crate::pac;
#[derive(Clone, Copy, Debug)]
pub struct ResetReason {
    pub raw_hbn: u8,
    pub watchdog: bool,
}
impl ResetReason {
    pub const fn power_on(self) -> bool {
        self.raw_hbn & 1 != 0
    }
    pub const fn external_pin(self) -> bool {
        self.raw_hbn & 2 != 0
    }
    pub const fn software(self) -> bool {
        self.raw_hbn & 4 != 0
    }
    pub const fn power_reset(self) -> bool {
        self.raw_hbn & 8 != 0
    }
    pub const fn brownout(self) -> bool {
        self.raw_hbn & 16 != 0
    }
}
pub fn reason() -> ResetReason {
    critical_section::with(|_| {
        pac::GLB.cgen_cfg1().modify(|r| r.set_timer(true));
        let raw_hbn = pac::HBN.HBN_GLB().read().hbn_reset_event();
        crate::watchdog::unlock();
        ResetReason {
            raw_hbn,
            watchdog: pac::TIMER.WSR().read().0 & 1 != 0,
        }
    })
}
pub fn clear_reason() {
    critical_section::with(|_| {
        pac::GLB.cgen_cfg1().modify(|r| r.set_timer(true));
        pac::HBN
            .HBN_GLB()
            .modify(|r| r.set_hbn_clear_reset_event(false));
        pac::HBN
            .HBN_GLB()
            .modify(|r| r.set_hbn_clear_reset_event(true));
        pac::HBN
            .HBN_GLB()
            .modify(|r| r.set_hbn_clear_reset_event(false));
        crate::watchdog::unlock();
        pac::TIMER.WSR().write(|r| r.0 = 0);
        crate::io_fence();
    });
}
/// System+CPU reset via the SDK sequence. Does not erase flash or request ROM ISP.
#[unsafe(link_section = ".text.flash_transaction")]
#[inline(never)]
pub fn system_reset() -> ! {
    unsafe {
        core::arch::asm!("csrci mstatus,8");
        crate::clock::switch_rc32m();
    }
    pac::GLB.swrst_cfg2().modify(|r| r.0 &= !7);
    crate::io_fence();
    pac::GLB.swrst_cfg2().modify(|r| r.0 |= 6);
    crate::io_fence();
    loop {
        core::hint::spin_loop();
    }
}
