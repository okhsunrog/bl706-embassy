//! RAM startup and CLIC trampolines. Only the software executor permits higher
//! level peripheral preemption. FPU builds preserve all f0..f31 at each level.

/// Last fatal trap (also usable before UART initialization), inspect with GDB.
#[repr(C)]
pub struct TrapRecord {
    pub mcause: u32,
    pub mepc: u32,
    pub mtval: u32,
}
#[unsafe(no_mangle)]
pub static mut BL706_TRAP: TrapRecord = TrapRecord {
    mcause: 0,
    mepc: 0,
    mtval: 0,
};

#[cfg(not(feature = "fpu"))]
core::arch::global_asm!(
    ".macro FP_INIT",
    ".endm",
    ".macro FP_SAVE",
    ".endm",
    ".macro FP_RESTORE",
    ".endm",
);
#[cfg(feature = "fpu")]
core::arch::global_asm!(include_str!("runtime-fpu.S"));
core::arch::global_asm!(
    include_str!("runtime.S"),
    msoft_irq = const crate::interrupt::MSOFT,
    mtime_irq = const crate::interrupt::MTIME,
    adc_irq = const crate::interrupt::ADC,
    uart0_irq = const crate::interrupt::UART0,
    i2c_irq = const crate::interrupt::I2C,
    pwm_irq = const crate::interrupt::PWM,
    timer0_irq = const crate::interrupt::TIMER0,
    timer1_irq = const crate::interrupt::TIMER1,
    gpio_irq = const crate::interrupt::GPIO,
);

#[cfg(feature = "flash")]
core::arch::global_asm!(include_str!("flash-boot.S"));
