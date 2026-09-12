/* RSTiny MicroPython port link script.
 *
 * Same runtime contract as the Rust/C apps (docs/sel4-abi.md): image base
 * 0x200000, page-aligned LOAD segments (flags 5 = R+X, 6 = R+W), `_start`
 * entry, per-page W^X. Exposes __sbss/__ebss for the rt0 .bss clear.
 */
ENTRY(_start)

PHDRS {
    text PT_LOAD FLAGS(5); /* R+X */
    data PT_LOAD FLAGS(6); /* R+W */
}

SECTIONS {
    . = 0x200000;
    .text : ALIGN(0x1000) {
        KEEP(*(.text.entry))
        *(.text .text.*)
    } :text
    .rodata : ALIGN(0x1000) {
        *(.rodata .rodata.*)
        *(.data.rel.ro .data.rel.ro.*)
    } :text
    . = ALIGN(0x1000);
    .data : ALIGN(0x1000) {
        *(.data .data.*)
    } :data
    .bss ALIGN(0x1000) (NOLOAD) : {
        __sbss = .;
        *(COMMON)
        *(.bss .bss.*)
        . = ALIGN(16);
        __ebss = .;
    } :data
    /DISCARD/ : {
        *(.comment)
        *(.note.gnu.property)
        *(.note.GNU-stack)
        *(.note.gnu.build-id)
        *(.eh_frame)
        *(.eh_frame_hdr)
        *(.gcc_except_table)
        *(.ARM.exidx)
        *(.ARM.attributes)
    }
}