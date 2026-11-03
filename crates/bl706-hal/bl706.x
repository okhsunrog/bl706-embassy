OUTPUT_ARCH(riscv)
ENTRY(_start)
PROVIDE(bl706_fatal_trap = bl706_default_trap);
MEMORY {
    ITCM (rwx) : ORIGIN = 0x22010000, LENGTH = 16K
    OCRAM (rwx) : ORIGIN = 0x22020000, LENGTH = 48K
}
/* Deliberately leave top 16K of OCRAM to reset/ROM EM allocation. RAM ELF,
   loaded by debugger; no flash header, boot2, cache setup or XIP assumption. */
SECTIONS {
    .init : ALIGN(4) { KEEP(*(.init)) KEEP(*(.text.clock_switch)) } > ITCM
    .vectors : ALIGN(64) { KEEP(*(.vectors)) } > ITCM
    .text : ALIGN(4) { *(.text .text.*) } > OCRAM
    .rodata : ALIGN(4) { *(.rodata .rodata.* .srodata .srodata.*) } > OCRAM
    .data : ALIGN(4) {
        __data_start = .;
        *(.data .data.* .sdata .sdata.*)
        . = ALIGN(4);
        __data_end = .;
    } > OCRAM
    PROVIDE(__global_pointer$ = __data_start + 0x800);
    .bss (NOLOAD) : ALIGN(4) {
        __bss_start = .;
        *(.bss .bss.* .sbss .sbss.* COMMON)
        . = ALIGN(4);
        __bss_end = .;
    } > OCRAM
    .stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .;
        . += 8K;
        __stack_top = .;
    } > OCRAM
    /DISCARD/ : { *(.eh_frame .eh_frame_hdr) }
}
ASSERT(__stack_top <= ORIGIN(OCRAM) + LENGTH(OCRAM), "OCRAM overflow")
ASSERT((bl706_vectors & 63) == 0, "CLIC vector table must be 64-byte aligned")
ASSERT((bl706_fatal_trap & 63) == 0, "CLIC mtvec BASE must be 64-byte aligned")
