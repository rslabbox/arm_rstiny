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
    . = 0x40080000;
    skernel = .;
    .text : {
        stext = .;
        KEEP(*(.text.boot))
        KEEP(*(.text.probe))
        *(.text .text.*)
        . = ALIGN(4096);
        etext = .;
    } :text
    .rodata : {
        srodata = .;
        *(.rodata .rodata.* .srodata .srodata.*)
        . = ALIGN(4096);
        erodata = .;
    } :rodata
    .data : {
        sdata = .;
        *(.data .data.* .sdata .sdata.* .got .got.*)
        . = ALIGN(4096);
        edata = .;
    } :data
    .bss (NOLOAD) : {
        sbss = .;
        *(.bss .bss.* .sbss .sbss.*)
        . = ALIGN(4096);
        ebss = .;
    } :data
    .stack (NOLOAD) : {
        stack_guard = .;
        . += 4096;
        boot_stack = .;
        . += 16K;
        boot_stack_top = .;
    } :data
    .heap (NOLOAD) : {
        __heap_start = .;
        . += 16M;
        __heap_end = .;
    } :data
    ekernel = .;

    ASSERT((sbss & 4095) == 0, "BSS alignment")
    ASSERT((boot_stack_top & 4095) == 0, "stack alignment")
    ASSERT(ekernel <= 0x42000000, "Kernel page tables cover only first 32 MiB of RAM")
    /DISCARD/ : { *(.eh_frame .eh_frame_hdr .comment) }
}
