.section .text.boot, "ax"
.global _start
.type _start, %function
_start:
    msr daifset, #0xf
    // Park all secondary affinities before they can touch shared BSS or stack.
    mrs x0, mpidr_el1
    ldr x1, =0xff00ffffff
    and x0, x0, x1
    cbnz x0, .Lpark
    mrs x19, CurrentEL
    cmp x19, #0x8
    b.eq .Lfrom_el2
    cmp x19, #0x4
    b.eq .Lel1
    // EL3/secure boot is deliberately unsupported by this platform contract.
    b .Lpark
.Lfrom_el2:
    mov x0, #(1 << 31)
    msr hcr_el2, x0
    mov x0, #3
    msr cnthctl_el2, x0
    msr cntvoff_el2, xzr
    msr cptr_el2, xzr
    ldr x0, =0x30d00800
    msr sctlr_el1, x0
    mov x0, #0x3c5
    msr spsr_el2, x0
    adr x0, .Lel1
    msr elr_el2, x0
    eret
.Lel1:
    msr spsel, #1
    ldr x0, =boot_stack_top
    mov sp, x0
    ldr x0, =exception_vector_base
    msr vbar_el1, x0
    isb
    // Entry contract requires translation and caches off before Rust starts.
    mrs x0, sctlr_el1
    tst x0, #1
    b.ne .Lpark
    tst x0, #4
    b.ne .Lpark
    tst x0, #0x1000
    b.ne .Lpark
    ldr x0, =sbss
    ldr x1, =ebss
.Lzero:
    cmp x0, x1
    b.hs .Lrust
    str xzr, [x0], #8
    b .Lzero
.Lrust:
    lsr x0, x19, #2
    bl boot_main
.Lpark:
    wfe
    b .Lpark
.size _start, . - _start
