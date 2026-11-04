//! Init-time-only RC32M/XTAL32M/DLL configuration. No live clock changes.
pub use crate::clock_logic::{Clocks, Config, Error, Reference, Root};
use crate::pac;

// ITCM only, no stack access while latching BCLK dividers. Move to a slow root first.
#[unsafe(naked)]
#[unsafe(link_section = ".text.clock_switch")]
#[unsafe(export_name = "bl706_clock_to_rc32m")]
pub(crate) unsafe extern "C" fn switch_rc32m() {
    core::arch::naked_asm!(
        "li t0,0x4000f030",
        "lw t1,0(t0)",
        "andi t1,t1,-4",
        "sw t1,0(t0)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "li t0,0x40000000",
        "lw t1,0(t0)",
        "li t2,0xff0000ff",
        "and t1,t1,t2",
        "ori t1,t1,14",
        "sw t1,0(t0)",
        "li t2,0x40000ffc",
        "li t3,1",
        "sw t3,0(t2)",
        "sw zero,0(t2)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "fence iorw,iorw",
        "ret"
    );
}
// a0=root encoding, a1=validated divisors. Set bus divider BEFORE selecting a fast root.
#[unsafe(naked)]
#[unsafe(link_section = ".text.clock_switch")]
#[unsafe(export_name = "bl706_clock_apply")]
unsafe extern "C" fn switch_planned(_root: u32, _dividers: u32) {
    core::arch::naked_asm!(
        "li t0,0x40000000",
        "lw t1,0(t0)",
        "li t2,0xff0000ff",
        "and t1,t1,t2",
        "or t1,t1,a1",
        "ori t1,t1,14",
        "sw t1,0(t0)",
        "li t2,0x40000ffc",
        "li t3,1",
        "sw t3,0(t2)",
        "sw zero,0(t2)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "li t0,0x4000f030",
        "lw t1,0(t0)",
        "andi t1,t1,-4",
        "or t1,t1,a0",
        "sw t1,0(t0)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "lw t1,0(t0)",
        "fence iorw,iorw",
        "ret"
    );
}
// Finite conservative delay while still on RC32M; loop overhead lengthens the pause.
fn rc_delay_us(us: u32) {
    for _ in 0..us * 32 {
        unsafe {
            core::arch::asm!("nop");
        }
    }
}
pub(crate) unsafe fn init(config: Config) -> Result<Clocks, Error> {
    let clocks = config.plan()?;
    unsafe {
        switch_rc32m();
    }
    if config.uses_xtal() {
        pac::AON.xtal_cfg().modify(|r| {
            r.set_xtal_capcode_in_aon(config.xtal_capcode_in);
            r.set_xtal_capcode_out_aon(config.xtal_capcode_out);
        });
        rc_delay_us(100); // SDK cap-code settling, including an already powered crystal.
        pac::AON.rf_top_aon().modify(|r| {
            r.set_pu_xtal_aon(true);
            r.set_pu_xtal_buf_aon(true);
        });
        let mut ready = false;
        for _ in 0..120 {
            rc_delay_us(10);
            if pac::AON.tsen().read().xtal_rdy() {
                ready = true;
                break;
            }
        }
        if !ready {
            return Err(Error::XtalTimeout);
        }
    }
    if let Root::Dll96m(reference) | Root::Dll144m(reference) = config.root {
        pac::GLB.dll().modify(|r| {
            r.set_ppu_dll(false);
            r.set_pu_dll(false);
            r.set_dll_reset(true);
        });
        pac::GLB.dll().modify(|r| {
            r.set_dll_refclk_sel(reference == Reference::Rc32m);
            r.set_dll_prechg_sel(true);
            r.set_ppu_dll(true);
        });
        rc_delay_us(2);
        pac::GLB.dll().modify(|r| r.set_pu_dll(true));
        rc_delay_us(2);
        pac::GLB.dll().modify(|r| r.set_dll_reset(false));
        rc_delay_us(5);
        // SDK settling sequence; there is no documented DLL lock-status bit here.
        pac::GLB.dll().modify(|r| {
            r.set_dll_clk_mmdiv_en(true);
            r.set_dll_clk_288M_en(true);
            r.set_dll_clk_96M_en(true);
            r.set_dll_clk_144M_en(true);
            r.set_dll_clk_57p6M_en(true);
        });
        pac::GLB.clk_cfg0().modify(|r| {
            r.set_reg_pll_en(true);
            r.set_reg_pll_sel(if matches!(config.root, Root::Dll96m(_)) {
                1
            } else {
                2
            });
        });
        pac::L1C.l1c_config().modify(|r| r.0 |= 1 << 12);
    }
    // Power/select RC32K explicitly; keep boot trim. SDK requires >800us startup pause.
    pac::HBN.HBN_GLB().modify(|r| {
        r.set_hbn_pu_rc32k(true);
        r.set_hbn_f32k_sel(0);
    });
    rc_delay_us(880);
    let dividers =
        (u32::from(config.cpu_divider - 1) << 8) | (u32::from(config.bus_divider - 1) << 16);
    unsafe {
        switch_planned(config.root_encoding(), dividers);
    }
    pac::GLB.clk_cfg2().modify(|r| r.set_uart_clk_en(false));
    pac::HBN.HBN_GLB().modify(|r| r.set_hbn_uart_clk_sel(false)); // FCLK, not XCLK.
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
    Ok(clocks)
}
