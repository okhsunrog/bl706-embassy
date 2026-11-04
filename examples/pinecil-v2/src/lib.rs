#![no_std]
use bl706_hal::gpio::Output;
use core::fmt::Write;

/// Nonblocking RTT output over the existing JTAG connection.
pub struct Logger;

impl Logger {
    /// Initialize once per boot, before clocks and peripherals.
    pub fn init() -> Self {
        rtt_target::rtt_init_print!(rtt_target::ChannelMode::NoBlockSkip, 4096);
        rtt_target::rprintln!("BL706 RAM entry; initializing HAL");
        Self
    }
}

impl core::fmt::Write for Logger {
    fn write_str(&mut self, text: &str) -> core::fmt::Result {
        rtt_target::rprint!("{}", text);
        Ok(())
    }
}

/// Hold the active-high heater drive low throughout each bring-up program.
pub struct Board {
    pub log: Logger,
    pub heater_off: Output<21>,
    pub i2c: bl706_hal::i2c::I2c,
    pub oled_reset: bl706_hal::gpio::Pin<3>,
    pub pd_irq: bl706_hal::gpio::Input<16>,
    pub key_a: bl706_hal::gpio::Input<25>,
    pub key_b: bl706_hal::gpio::Input<28>,
}
pub fn board() -> Board {
    let log = Logger::init();
    let p = bl706_hal::init();
    let heater_off = p.pins.p21.into_output(false);
    let board = Board {
        log,
        heater_off,
        i2c: bl706_hal::i2c::I2c::new(p.i2c, p.pins.p10, p.pins.p11, p.clocks),
        oled_reset: p.pins.p3,
        pd_irq: p.pins.p16.into_input(bl706_hal::gpio::Pull::Up),
        key_a: p.pins.p25.into_input(bl706_hal::gpio::Pull::Down),
        key_b: p.pins.p28.into_input(bl706_hal::gpio::Pull::Down),
    };
    bl706_hal::enable_interrupts();
    board
}
pub fn banner(log: &mut Logger, name: &str) {
    let misa: usize;
    unsafe { core::arch::asm!("csrr {}, misa", out(reg) misa) };
    let _ = writeln!(
        log,
        "\r\nBL706 {name}: nominal RC32M, RAM ELF, misa={misa:08x}\r"
    );
}
#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    unsafe { core::arch::asm!("csrci mstatus, 8") };
    rtt_target::rprintln!("panic: {}", info);
    loop {
        core::hint::spin_loop();
    }
}
