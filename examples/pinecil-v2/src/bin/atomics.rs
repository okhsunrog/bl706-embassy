//! Deliberately contains A instructions in isolated, bounded probes only.
//! Hardware results are unknown. A normal HAL program must never use these.
#![no_std]
#![no_main]
use core::{
    fmt::Write,
    ptr::{addr_of_mut, read_volatile},
};
use pinecil_v2_bringup::{banner, board};

#[unsafe(no_mangle)]
static mut PROBE_WORD: u32 = 0x1234_5678;
unsafe extern "C" {
    fn bl706_probe_lrsc(p: *mut u32) -> u32;
    fn bl706_probe_amo(p: *mut u32) -> u32;
    static BL706_TRAP: [u32; 3];
}
#[unsafe(no_mangle)]
extern "C" fn __bl706_main() -> ! {
    let mut b = board();
    banner(
        &mut b.uart,
        "bounded atomic probes; NO general atomic support assumed",
    );
    // No interrupts or DMA race the test word. Each instruction is attempted
    // once; recovery is limited to these exact labelled PCs.
    unsafe { core::arch::asm!("csrci mstatus, 8") };
    let misa: usize;
    let marchid: usize;
    let mimpid: usize;
    unsafe {
        core::arch::asm!("csrr {}, misa", out(reg) misa);
        core::arch::asm!("csrr {}, marchid", out(reg) marchid);
        core::arch::asm!("csrr {}, mimpid", out(reg) mimpid);
    }
    let p = addr_of_mut!(PROBE_WORD);
    let _ = writeln!(
        b.uart,
        "misa={misa:08x} marchid={marchid:08x} mimpid={mimpid:08x} word={:08x}\r",
        p as usize
    );
    let _ = b.uart.write(b"LR/SC: attempting once\r\n");
    let sc = unsafe { bl706_probe_lrsc(p) };
    let record = unsafe { read_volatile(&raw const BL706_TRAP) };
    let value = unsafe { read_volatile(p) };
    let _ = writeln!(
        b.uart,
        "LR/SC status={sc:08x} word={value:08x} trap={record:08x?}\r"
    );
    let _ = b.uart.write(b"AMOADD: attempting once\r\n");
    let old = unsafe { bl706_probe_amo(p) };
    let record = unsafe { read_volatile(&raw const BL706_TRAP) };
    let value = unsafe { read_volatile(p) };
    let _ = writeln!(
        b.uart,
        "AMO old={old:08x} word={value:08x} trap={record:08x?}\r"
    );
    let _ = b
        .uart
        .write(b"Probe complete. Results apply only to this word/alias/revision.\r\n");
    loop {
        core::hint::spin_loop();
    }
}

core::arch::global_asm!(
    r#"
.section .text.atomic_probe, "ax"
.balign 4
.global bl706_probe_lrsc
bl706_probe_lrsc:
    la t0, BL706_TRAP
    sw zero, 0(t0)
    sw zero, 4(t0)
    sw zero, 8(t0)
    li a1, 0x87654321
    .option push
    .option arch, +a
.global bl706_probe_lr
bl706_probe_lr:
    lr.w a2, (a0)
.global bl706_probe_sc
bl706_probe_sc:
    sc.w a0, a1, (a0)
    .option pop
    ret
.global bl706_probe_amo
bl706_probe_amo:
    la t0, BL706_TRAP
    sw zero, 0(t0)
    sw zero, 4(t0)
    sw zero, 8(t0)
    li a1, 1
    .option push
    .option arch, +a
.global bl706_probe_amo_insn
bl706_probe_amo_insn:
    amoadd.w a0, a1, (a0)
    .option pop
    ret
bl706_probe_failed:
    li a0, -1
    ret

# Overrides the linker-provided default ONLY in this diagnostic binary.
.balign 64
.global bl706_fatal_trap
bl706_fatal_trap:
    csrr t0, mcause
    csrr t1, mepc
    csrr t2, mtval
    la t3, BL706_TRAP
    sw t0, 0(t3)
    sw t1, 4(t3)
    sw t2, 8(t3)
    # Only synchronous illegal instruction (2), load fault (5), or
    # store/AMO access fault (7) at one of the three exact probe instructions.
    li t3, 2
    beq t0, t3, 1f
    li t3, 5
    beq t0, t3, 1f
    li t3, 7
    bne t0, t3, 9f
1:  la t3, bl706_probe_lr
    beq t1, t3, 2f
    la t3, bl706_probe_sc
    beq t1, t3, 2f
    la t3, bl706_probe_amo_insn
    bne t1, t3, 9f
2:  la t0, bl706_probe_failed
    csrw mepc, t0
    mret
9:  csrci mstatus, 8
    j 9b
"#
);
