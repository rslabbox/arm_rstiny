.section .text.boot
.global _start

_start:
    mov x8, #97
    mov x9, #0x09000000 //串口地址，需要变化
    str x8, [x9]
    mov x8, #10
    str x8, [x9]

    msr daifset, #2   // 关闭所有中断

    adrp    x0, exception_vector_base
    add     x0, x0, :lo12:exception_vector_base
    msr     vbar_el1, x0
    dsb     sy      // 确保所有内存访问完成
    isb             // 确保所有指令都执行完成

    // 设置栈指针
    ldr x0, =__stack_end
    mov sp, x0
    
    // 清零 BSS 段
    ldr x0, =__bss_start
    ldr x1, =__bss_end
    mov x2, #0
clear_bss:
    cmp x0, x1
    b.ge clear_bss_done
    str x2, [x0], #8
    b clear_bss
clear_bss_done:

    bl rust_main
    
halt:
    wfe
    b halt
