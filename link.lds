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
    KERNEL_OFFSET = 0xffff000000000000;
    . = KERNEL_OFFSET + 0x40200000;
    skernel = .;
    .text : AT(ADDR(.text) - KERNEL_OFFSET) {
        stext = .;
        KEEP(*(.text.boot))
        KEEP(*(.text.probe))
        *(.text .text.*)
        . = ALIGN(4096);
        etext = .;
    } :text
    .rodata : AT(ADDR(.rodata) - KERNEL_OFFSET) {
        srodata = .;
        *(.rodata .rodata.* .srodata .srodata.*)
        . = ALIGN(4096);
        erodata = .;
    } :rodata
    .data : AT(ADDR(.data) - KERNEL_OFFSET) {
        sdata = .;
        *(.data .data.* .sdata .sdata.* .got .got.*)
        . = ALIGN(4096);
        edata = .;
    } :data
    .bss (NOLOAD) : AT(ADDR(.bss) - KERNEL_OFFSET) {
        sbss = .;
        *(.bss .bss.* .sbss .sbss.*)
        . = ALIGN(4096);
        ebss = .;
    } :data
    .stack (NOLOAD) : AT(ADDR(.stack) - KERNEL_OFFSET) {
        stack_guard = .;
        . += 4096;
        boot_stack = .;
        . += 64K;
        boot_stack_top = .;
    } :data
    .heap (NOLOAD) : AT(ADDR(.heap) - KERNEL_OFFSET) {
        __heap_start = .;
        . += 16M;
        __heap_end = .;
    } :data
    .frames (NOLOAD) : AT(ADDR(.frames) - KERNEL_OFFSET) {
        __frames_start = .;
        . += 8M;
        __frames_end = .;
    } :data
    ekernel = .;

    ASSERT((sbss & 4095) == 0, "BSS alignment")
    ASSERT((boot_stack_top & 4095) == 0, "stack alignment")
    ASSERT((ekernel - KERNEL_OFFSET) <= 0x42000000, "Kernel page tables cover only first 32 MiB of RAM")
    /DISCARD/ : { *(.eh_frame .eh_frame_hdr .comment) }
}
