.section .text.boot
.global _start

_start:
    mov x8, #97
    mov x9, #0x09000000 //串口地址，需要变化
    str x8, [x9]
    mov x8, #10
    str x8, [x9]

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

    // 跳转到 Rust main 函数
    bl rust_main
    
    // 如果 main 返回，进入无限循环
halt:
    wfe
    b halt
