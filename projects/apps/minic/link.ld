/* minic C app link script (interpreter-app.md 决策 F).
 *
 * Runtime contract shared with the Rust apps:
 *   - fixed image base 0x200000,
 *   - page-aligned LOAD segments the ELF loader validates
 *     (flags 4..=6, va/offset % 4096 == 0, no overlap, entry inside an
 *     executable segment),
 *   - `_start` entry.
 * Per-page W^X: code+rodata read/execute, data+bss read/write.
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
    } :text
    . = ALIGN(0x1000);
    .data : ALIGN(0x1000) {
        *(.data .data.*)
    } :data
    .bss ALIGN(0x1000) (NOLOAD) : {
        *(COMMON)
        *(.bss .bss.*)
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