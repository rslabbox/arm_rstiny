.macro SAVE_REGS
    sub     sp, sp, 34 * 8
    stp     x0, x1, [sp]
    stp     x2, x3, [sp, 2 * 8]
    stp     x4, x5, [sp, 4 * 8]
    stp     x6, x7, [sp, 6 * 8]
    stp     x8, x9, [sp, 8 * 8]
    stp     x10, x11, [sp, 10 * 8]
    stp     x12, x13, [sp, 12 * 8]
    stp     x14, x15, [sp, 14 * 8]
    stp     x16, x17, [sp, 16 * 8]
    stp     x18, x19, [sp, 18 * 8]
    stp     x20, x21, [sp, 20 * 8]
    stp     x22, x23, [sp, 22 * 8]
    stp     x24, x25, [sp, 24 * 8]
    stp     x26, x27, [sp, 26 * 8]
    stp     x28, x29, [sp, 28 * 8]

    mrs     x9, sp_el0
    mrs     x10, elr_el1
    mrs     x11, spsr_el1
    stp     x30, x9, [sp, 30 * 8]
    stp     x10, x11, [sp, 32 * 8]
.endm

// Keep each vector slot to one branch. Saving the frame happens out of line.
.section .text, "ax"
.p2align 11
.global exception_vector_base
exception_vector_base:
    b vector_0_0
    .balign 128
    b vector_0_1
    .balign 128
    b vector_0_2
    .balign 128
    b vector_0_3
    .balign 128
    b vector_1_0
    .balign 128
    b vector_1_1
    .balign 128
    b vector_1_2
    .balign 128
    b vector_1_3
    .balign 128
    b vector_2_0
    .balign 128
    b vector_2_1
    .balign 128
    b vector_2_2
    .balign 128
    b vector_2_3
    .balign 128
    b vector_3_0
    .balign 128
    b vector_3_1
    .balign 128
    b vector_3_2
    .balign 128
    b vector_3_3
    .balign 128
vector_0_0:
    SAVE_REGS
    mov x0, sp
    mov x1, #0
    mov x2, #0
    bl fatal_exception
    b .
vector_0_1:
    SAVE_REGS
    mov x0, sp
    mov x1, #1
    mov x2, #0
    bl fatal_exception
    b .
vector_0_2:
    SAVE_REGS
    mov x0, sp
    mov x1, #2
    mov x2, #0
    bl fatal_exception
    b .
vector_0_3:
    SAVE_REGS
    mov x0, sp
    mov x1, #3
    mov x2, #0
    bl fatal_exception
    b .
vector_1_0:
    SAVE_REGS
    mov x0, sp
    mov x1, #0
    mov x2, #1
    bl fatal_exception
    b .
vector_1_1:
    SAVE_REGS
    mov x0, sp
    bl handle_irq
    b .
vector_1_2:
    SAVE_REGS
    mov x0, sp
    mov x1, #2
    mov x2, #1
    bl fatal_exception
    b .
vector_1_3:
    SAVE_REGS
    mov x0, sp
    mov x1, #3
    mov x2, #1
    bl fatal_exception
    b .
vector_2_0:
    SAVE_REGS
    mov x0, sp
    bl handle_user_sync
    b .
vector_2_1:
    SAVE_REGS
    mov x0, sp
    bl handle_irq
    b .
vector_2_2:
    SAVE_REGS
    mov x0, sp
    mov x1, #2
    mov x2, #2
    bl fatal_exception
    b .
vector_2_3:
    SAVE_REGS
    mov x0, sp
    mov x1, #3
    mov x2, #2
    bl fatal_exception
    b .
vector_3_0:
    SAVE_REGS
    mov x0, sp
    mov x1, #0
    mov x2, #3
    bl fatal_exception
    b .
vector_3_1:
    SAVE_REGS
    mov x0, sp
    mov x1, #1
    mov x2, #3
    bl fatal_exception
    b .
vector_3_2:
    SAVE_REGS
    mov x0, sp
    mov x1, #2
    mov x2, #3
    bl fatal_exception
    b .
vector_3_3:
    SAVE_REGS
    mov x0, sp
    mov x1, #3
    mov x2, #3
    bl fatal_exception
    b .
