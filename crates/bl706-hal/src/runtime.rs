//! RAM startup and non-nested CLIC trampolines. FPU builds preserve all f0..f31.

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
core::arch::global_asm!(include_str!("runtime.S"));
