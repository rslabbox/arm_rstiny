OUTPUT_ARCH(aarch64)
ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}

SECTIONS
{
    KERNEL_LINK_BASE = 0xffff800000000000;
    . = KERNEL_LINK_BASE;
    skernel = .;
    .text : AT(ADDR(.text) - KERNEL_LINK_BASE) {
        stext = .;
        KEEP(*(.text.boot))
        KEEP(*(.text.probe))
        *(.text .text.*)
        . = ALIGN(4096);
        etext = .;
    } :text
    .rodata : AT(ADDR(.rodata) - KERNEL_LINK_BASE) {
        srodata = .;
        *(.rodata .rodata.* .srodata .srodata.*)
        . = ALIGN(4096);
        erodata = .;
    } :rodata
    .data : AT(ADDR(.data) - KERNEL_LINK_BASE) {
        sdata = .;
        *(.data .data.* .sdata .sdata.* .got .got.*)
        . = ALIGN(4096);
        edata = .;
    } :data
    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_LINK_BASE) {
        sbss = .;
        *(.bss .bss.* .sbss .sbss.*)
        . = ALIGN(4096);
        ebss = .;
    } :data
    .stack (NOLOAD) : AT(ADDR(.stack) - KERNEL_LINK_BASE) {
        stack_guard = .;
        . += 4096;
        boot_stack = .;
        . += 64K;
        boot_stack_top = .;
    } :data
    .heap (NOLOAD) : AT(ADDR(.heap) - KERNEL_LINK_BASE) {
        __heap_start = .;
        . += 16M;
        __heap_end = .;
    } :data
    .frames (NOLOAD) : AT(ADDR(.frames) - KERNEL_LINK_BASE) {
        __frames_start = .;
        . += 8M;
        __frames_end = .;
    } :data
    ekernel = .;

    ASSERT((sbss & 4095) == 0, "BSS alignment")
    ASSERT((boot_stack_top & 4095) == 0, "stack alignment")
    ASSERT((ekernel - skernel) <= 0x2000000, "Kernel image exceeds its 32 MiB virtual window")
    /DISCARD/ : { *(.eh_frame .eh_frame_hdr .comment) }
}
