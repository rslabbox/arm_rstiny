/* minic entry (interpreter-app.md 决策 F).
 *
 * EL0 loader contract: x0 = parameter page VA, SP = 64 KiB stack top,
 * all mappings W^X. Port rt0: hand the parameter page to the C main and
 * never return (the program exits through the control endpoint).
 */
    .section .text.entry, "ax", @progbits
    .globl _start
    .type _start, %function
_start:
    mov     x29, #0
    b       port_main
    .size _start, .-_start
    .section .note.GNU-stack, "", @progbits