// startup.S
.section .text
.global _start

.section .text.boot, "x"
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
    ldr x0, =_stack_top
    mov sp, x0

    // 调用 C 语言的 main 函数
    bl rust_main

    // 死循环，防止返回
1:  b 1b

// 栈空间
.section .bss
.align 12
.global _stack_top
_stack_top:
    .skip 0x8000  // 8KB 的栈空间
