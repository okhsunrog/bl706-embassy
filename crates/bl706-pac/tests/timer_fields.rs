//! BL702 RM TCDR: [7:0] reserved, [15:8] TIMER2, [23:16] TIMER3, [31:24] WDT.
use bl706_pac::timer::regs::TCDR;

#[test]
fn timer_divisors_do_not_touch_sibling_watchdog_or_reserved_byte() {
    let mut register = TCDR(0x1f_37_53_a5);
    register.set_tcdr2(143);
    assert_eq!(register.0, 0x1f_37_8f_a5);
    register.set_tcdr3(71);
    assert_eq!(register.0, 0x1f_47_8f_a5);
    assert_eq!(register.wcdr(), 31);
}
