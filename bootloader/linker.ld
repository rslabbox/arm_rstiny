OUTPUT_ARCH(aarch64)
ENTRY(_start)
PHDRS {
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}
SECTIONS {
    . = 0x44000000;
    __loader_start = .;
    .text : { KEEP(*(.text.boot)) *(.text .text.*) } :text
    . = ALIGN(4096);
    .rodata : { *(.rodata .rodata.*) } :rodata
    .boot_archive : {
        . = ALIGN(4);
        __archive_start = .;
        KEEP(*(.boot_archive))
        __archive_end = .;
    } :rodata
    ASSERT(__archive_end > __archive_start, "missing boot archive: build with make build or set BOOT_ARCHIVE_OBJECT")
    . = ALIGN(4096);
    .data : { *(.data .data.*) *(.got .got.*) } :data
    .bss (NOLOAD) : {
        . = ALIGN(4096);
        __bss_start = .;
        *(.bss .bss.*) *(COMMON)
        . = ALIGN(16);
        __bss_end = .;
    } :data
    .stack (NOLOAD) : {
        . = ALIGN(4096);
        . += 65536;
        __stack_top = .;
    } :data
    __loader_end = .;
    ASSERT(__loader_end <= 0x48000000, "bootloader exceeds QEMU RAM")
    /DISCARD/ : { *(.eh_frame*) *(.comment) }
}
