//! Conservative nominal 32 MHz RC clock; PLL/XIP retiming is out of scope.
use crate::pac;
#[derive(Clone, Copy, Debug)]
pub struct Clocks {
    pub(crate) _private: (),
}
impl Clocks {
    pub const fn cpu_hz(self) -> u32 {
        32_000_000
    }
    pub const fn bus_hz(self) -> u32 {
        32_000_000
    }
    pub const fn uart_hz(self) -> u32 {
        32_000_000
    }
}

// Divider transition executes from ITCM without stack accesses. See SDK
// GLB_Set_System_CLK/Div and its OCRAM warning. Preserve the SDK dummy reads.
#[unsafe(naked)]
#[unsafe(link_section = ".text.clock_switch")]
unsafe extern "C" fn switch_rc32m() {
    core::arch::naked_asm!(
        "li t0, 0x40000000",
        "lw t1, 0(t0)",
        "ori t1, t1, 14",
        "sw t1, 0(t0)",
        "li t2, 0x4000f030",
        "lw t3, 0(t2)",
        "li t4, -4",
        "and t3, t3, t4",
        "sw t3, 0(t2)",
        "lw t3, 0(t2)",
        "lw t3, 0(t2)",
        "lw t3, 0(t2)",
        "lw t3, 0(t2)",
        "lw t1, 0(t0)",
        "li t2, 0xff0000ff",
        "and t1, t1, t2",
        "sw t1, 0(t0)",
        "li t2, 0x40000ffc",
        "li t3, 1",
        "sw t3, 0(t2)",
        "sw zero, 0(t2)",
        "lw t1, 0(t0)",
        "lw t1, 0(t0)",
        "lw t1, 0(t0)",
        "lw t1, 0(t0)",
        "fence iorw, iorw",
        "ret",
    )
}
pub(crate) unsafe fn init() -> Clocks {
    unsafe { switch_rc32m() };
    pac::GLB.clk_cfg2().modify(|r| r.set_uart_clk_en(false));
    pac::HBN.HBN_GLB().modify(|r| r.set_hbn_uart_clk_sel(false));
    pac::GLB.clk_cfg2().modify(|r| {
        r.set_uart_clk_div(0);
        r.set_uart_clk_en(true);
    });
    pac::GLB.clk_cfg3().modify(|r| {
        r.set_i2c_clk_div(0);
        r.set_i2c_clk_en(true);
    });
    pac::GLB.cgen_cfg1().modify(|r| {
        r.set_uart0(true);
        r.set_i2c(true);
    });
    Clocks { _private: () }
}
