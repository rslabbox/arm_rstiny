/* RSTiny EL0 entry (docs/micropython-port.md §6).
 *
 * EL0 loader contract (docs/sel4-abi.md): x0 = parameter page VA, SP set to
 * the 64 KiB stack top, mappings W^X. rt0 zeroes .bss (the loader maps
 * NOLOAD sections but does not clear them), then hands the parameter page to
 * the C startup. The program never returns: it exits through the control
 * endpoint.
 */
    .section .text.entry, "ax", @progbits
    .globl _start
    .type _start, %function
_start:
    /* zero .bss (gc heap, static state) */
    adrp    x9, __sbss
    add     x9, x9, :lo12:__sbss
    adrp    x10, __ebss
    add     x10, x10, :lo12:__ebss
1:
    cmp     x9, x10
    b.hs    2f
    str     xzr, [x9], #8
    b       1b
2:
    mov     x29, #0
    b       rstiny_start
    .size _start, .-_start
    .section .note.GNU-stack, "", @progbits