OUTPUT_ARCH(riscv)
ENTRY(_flash_start)
PROVIDE(bl706_fatal_trap = bl706_default_trap);
MEMORY {
    /* Metadata VMA is never accessed; its load address is physical flash 0. */
    HEADER (r) : ORIGIN = 0x21000000, LENGTH = 176
    XIP (rx) : ORIGIN = 0x23000000, LENGTH = 4K
    FLASH (r) : ORIGIN = 0x23001000, LENGTH = 1M - 4K
    /* ROM flash startup reserves the first 16 KiB ILM window. */
    ITCM (rwx) : ORIGIN = 0x22014000, LENGTH = 16K
    OCRAM (rwx) : ORIGIN = 0x22020000, LENGTH = 48K
}
SECTIONS {
    .bootheader : AT(0x23000000) { KEEP(*(.bootheader)) } > HEADER
    .flash_boot : ALIGN(4) { KEEP(*(.flash_boot)) . = ALIGN(4); } > XIP AT> FLASH
    .init : ALIGN(64) {
        __tcm_start = .;
        KEEP(*(.init)) KEEP(*(.text.clock_switch))
    } > ITCM AT> FLASH
    .vectors : ALIGN(64) {
        KEEP(*(.vectors))
        . = ALIGN(4); __tcm_end = .;
    } > ITCM AT> FLASH
    .text : ALIGN(64) {
        __ocram_start = .;
        *(.text .text.*)
    } > OCRAM AT> FLASH
    .rodata : ALIGN(4) { *(.rodata .rodata.* .srodata .srodata.*) } > OCRAM AT> FLASH
    .data : ALIGN(4) {
        __data_start = .;
        *(.data .data.* .sdata .sdata.*)
        . = ALIGN(4); __data_end = .;
        __ocram_end = .;
    } > OCRAM AT> FLASH
    PROVIDE(__global_pointer$ = __data_start + 0x800);
    __tcm_load_xip = LOADADDR(.init) - 0x1000;
    __ocram_load_xip = LOADADDR(.text) - 0x1000;
    .bss (NOLOAD) : ALIGN(4) {
        __bss_start = .;
        *(.bss .bss.* .sbss .sbss.* COMMON)
        . = ALIGN(4); __bss_end = .;
    } > OCRAM
    .stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .; . += 8K; __stack_top = .;
    } > OCRAM
    /DISCARD/ : { *(.eh_frame .eh_frame_hdr) }
}
ASSERT(SIZEOF(.bootheader) == 176, "BL702 boot header must be 176 bytes")
ASSERT(__stack_top <= ORIGIN(OCRAM) + LENGTH(OCRAM), "OCRAM overflow")
ASSERT((bl706_vectors & 63) == 0, "CLIC vector table must be 64-byte aligned")
ASSERT((bl706_fatal_trap & 63) == 0, "CLIC mtvec BASE must be 64-byte aligned")
ASSERT(LOADADDR(.vectors) - LOADADDR(.init) == ADDR(.vectors) - ADDR(.init), "TCM copy layout mismatch")
ASSERT(LOADADDR(.rodata) - LOADADDR(.text) == ADDR(.rodata) - ADDR(.text), "rodata copy layout mismatch")
ASSERT(LOADADDR(.data) - LOADADDR(.text) == ADDR(.data) - ADDR(.text), "data copy layout mismatch")
