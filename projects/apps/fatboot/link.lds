OUTPUT_ARCH(aarch64)
ENTRY(_start)
PHDRS {
    text PT_LOAD FLAGS(5);
    rodata PT_LOAD FLAGS(4);
    data PT_LOAD FLAGS(6);
}
SECTIONS {
    . = 0x00400000;
    .text : { KEEP(*(.text.entry)) *(.text .text.*) . = ALIGN(4096); } :text
    .rodata : { *(.rodata .rodata.*) . = ALIGN(4096); } :rodata
    .data : { *(.data .data.* .got .got.*) . = ALIGN(4096); } :data
    .bss (NOLOAD) : { *(.bss .bss.*) . = ALIGN(4096); } :data
    ASSERT(. <= 0x00500000, "fatboot image exceeds boot ABI image window")
    /DISCARD/ : { *(.eh_frame .eh_frame_hdr .comment) }
}
