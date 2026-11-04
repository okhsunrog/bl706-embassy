/* ROM configures XIP, then this stub copies the existing RAM image. */
.section .bootheader, "a"
.balign 4
.global BL706_BOOT_HEADER
BL706_BOOT_HEADER:
    .zero 176

.section .flash_boot, "ax"
.balign 4
.global _flash_start
_flash_start:
    .option push
    .option norelax
    csrci mstatus, 8
    /* No stack, calls, interrupts, or clock changes before the copies finish. */
    la t0, __tcm_load_xip
    la t1, __tcm_start
    la t2, __tcm_end
1:  bgeu t1, t2, 2f
    lw t3, 0(t0)
    sw t3, 0(t1)
    addi t0, t0, 4
    addi t1, t1, 4
    j 1b
2:  la t0, __ocram_load_xip
    la t1, __ocram_start
    la t2, __ocram_end
3:  bgeu t1, t2, 4f
    lw t3, 0(t0)
    sw t3, 0(t1)
    addi t0, t0, 4
    addi t1, t1, 4
    j 3b
4:  fence iorw, iorw
    fence.i
    la t0, _start
    jr t0
    .option pop
