ENTRY(_start)

MEMORY
{
    RAM : ORIGIN = 0x40000000, LENGTH = 128M
}

SECTIONS
{
    . = 0x40080000;
    
    .text : {
        KEEP(*(.text.boot))
        *(.text*)
    } > RAM
    
    .rodata : {
        *(.rodata*)
    } > RAM
    
    .data : {
        *(.data*)
    } > RAM
    
    .bss : {
        __bss_start = .;
        *(.bss*)
        *(COMMON)
        __bss_end = .;
    } > RAM
    
    . = ALIGN(8);
    __heap_start = .;
    . += 0x1000000; /* 16MB heap */
    __heap_end = .;
    
    . = ALIGN(8);
    __stack_start = .;
    . += 0x100000; /* 1MB stack */
    __stack_end = .;
    
    /DISCARD/ : {
        *(.comment)
        *(.gnu*)
        *(.note*)
        *(.eh_frame*)
    }
}
