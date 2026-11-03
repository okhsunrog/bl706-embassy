#![no_std]
use bl706_hal::{gpio::Output, uart::Uart0};
use core::fmt::Write;

/// Hold the active-high heater drive low throughout each bring-up program.
pub struct Board {
    pub uart: Uart0,
    pub heater_off: Output<21>,
    pub i2c: bl706_hal::i2c::I2c,
    pub pd_irq: bl706_hal::gpio::Input<16>,
    pub key_a: bl706_hal::gpio::Input<25>,
    pub key_b: bl706_hal::gpio::Input<28>,
}
pub fn board() -> Board {
    let p = bl706_hal::init();
    let heater_off = p.pins.p21.into_output(false);
    let uart = Uart0::new(p.uart0, p.pins.p22, p.clocks, 115_200).unwrap();
    let board = Board {
        uart,
        heater_off,
        i2c: bl706_hal::i2c::I2c::new(p.i2c, p.pins.p10, p.pins.p11, p.clocks),
        pd_irq: p.pins.p16.into_input(bl706_hal::gpio::Pull::Up),
        key_a: p.pins.p25.into_input(bl706_hal::gpio::Pull::Down),
        key_b: p.pins.p28.into_input(bl706_hal::gpio::Pull::Down),
    };
    bl706_hal::enable_interrupts();
    board
}
pub fn banner(uart: &mut Uart0, name: &str) {
    let misa: usize;
    unsafe { core::arch::asm!("csrr {}, misa", out(reg) misa) };
    let _ = writeln!(
        uart,
        "\r\nBL706 {name}: nominal RC32M, RAM ELF, misa={misa:08x}\r"
    );
}
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    unsafe { core::arch::asm!("csrci mstatus, 8") };
    loop {
        core::hint::spin_loop();
    }
}
