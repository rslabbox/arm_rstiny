#[unsafe(naked)]
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.boot")]
unsafe extern "C" fn _start() -> ! {
    core::arch::naked_asm!("
        // Uart0 = 0xfeb50000
        mov x9, #0x0
        movk x9, #0xfeb5, lsl #16
        // Hi
        mov x10, #72
        str x10, [x9]
        mov x10, #105
        str x10, [x9]
        mov x10, #10
        str x10, [x9]

        adrp    x8, boot_stack_top
        mov     sp, x8

        ldr     x8, ={rust_main}
        blr     x8
        b      .",
        rust_main = sym crate::rust_main,
    )
}
