.section .init, "ax"
.balign 4
.global _start
_start:
    csrci mstatus, 8
    .option push
    .option norelax
    la gp, __global_pointer$
    .option pop
    la sp, __stack_top
    la t0, bl706_fatal_trap
    ori t0, t0, 3
    csrw mtvec, t0
    la t0, bl706_vectors
    csrw 0x307, t0
    # RAM ELF: debugger loads .data at its VMA; only .bss needs initialization.
    la t0, __bss_start
    la t1, __bss_end
1:  bgeu t0, t1, 2f
    sw zero, 0(t0)
    addi t0, t0, 4
    j 1b
2:
    FP_INIT
    fence.i
    call __bl706_main
    j bl706_fatal_trap

.section .vectors, "a"
.balign 64
.global bl706_vectors
bl706_vectors:
    .rept 7
    .word bl706_fatal_trap
    .endr
    .word bl706_irq_entry
    .rept 40
    .word bl706_fatal_trap
    .endr
    .word bl706_irq_entry
    .rept 31
    .word bl706_fatal_trap
    .endr

.section .text.trap, "ax"
.balign 64
.global bl706_default_trap
bl706_default_trap:
    csrci mstatus, 8
    la t3, BL706_TRAP
    csrr t0, mcause
    csrr t1, mepc
    csrr t2, mtval
    sw t0, 0(t3)
    sw t1, 4(t3)
    sw t2, 8(t3)
3:  j 3b

.balign 4
bl706_irq_entry:
    # 224-byte, 16-aligned frame: integer caller-saved regs, mstatus, FCSR, FPRs.
    addi sp, sp, -224
    sw ra, 0(sp)
    sw t0, 4(sp)
    sw t1, 8(sp)
    sw t2, 12(sp)
    sw a0, 16(sp)
    sw a1, 20(sp)
    sw a2, 24(sp)
    sw a3, 28(sp)
    sw a4, 32(sp)
    sw a5, 36(sp)
    sw a6, 40(sp)
    sw a7, 44(sp)
    sw t3, 48(sp)
    sw t4, 52(sp)
    sw t5, 56(sp)
    sw t6, 60(sp)
    csrr t0, mstatus
    sw t0, 68(sp)
    FP_SAVE
    csrr a0, mcause
    andi a0, a0, 0xff
    call bl706_irq_dispatch
    FP_RESTORE
    lw t0, 68(sp)
    csrw mstatus, t0
    lw ra, 0(sp)
    lw t0, 4(sp)
    lw t1, 8(sp)
    lw t2, 12(sp)
    lw a0, 16(sp)
    lw a1, 20(sp)
    lw a2, 24(sp)
    lw a3, 28(sp)
    lw a4, 32(sp)
    lw a5, 36(sp)
    lw a6, 40(sp)
    lw a7, 44(sp)
    lw t3, 48(sp)
    lw t4, 52(sp)
    lw t5, 56(sp)
    lw t6, 60(sp)
    addi sp, sp, 224
    mret
