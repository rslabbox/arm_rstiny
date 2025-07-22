#![no_std]
#![no_main]
#![feature(alloc_error_handler)]

extern crate alloc;
use alloc::vec::Vec;
use core::{arch::asm, panic::PanicInfo};

mod allocator;
mod console;

// 引入汇编启动代码
core::arch::global_asm!(include_str!("boot.s"));

#[no_mangle]
pub extern "C" fn rust_main() -> ! {
    println!("ARM RSTiny2 - Rust Bare Metal OS");
    println!("================================");

    // 初始化堆内存分配器
    println!("Initializing heap allocator...");
    allocator::init_heap();
    println!("Heap allocator initialized successfully!");

    // 测试内存分配 - 创建 Vec
    println!("\nTesting memory allocation with Vec:");

    let mut numbers: Vec<i32> = Vec::new();
    println!("Created empty Vec");

    // 向 Vec 中添加元素
    for i in 1..=10 {
        numbers.push(i * i);
        println!("Added {} to Vec, current length: {}", i * i, numbers.len());
    }

    println!("\nVec contents:");
    for (index, value) in numbers.iter().enumerate() {
        println!("  numbers[{}] = {}", index, value);
    }

    println!("\nVec capacity: {}", numbers.capacity());
    println!("Vec length: {}", numbers.len());

    // 测试更多内存分配
    println!("\nCreating another Vec with strings:");
    let mut strings: Vec<&str> = Vec::new();
    strings.push("Hello");
    strings.push("from");
    strings.push("Rust");
    strings.push("bare");
    strings.push("metal");
    strings.push("OS!");

    print!("Message: ");
    for (i, s) in strings.iter().enumerate() {
        if i > 0 {
            print!(" ");
        }
        print!("{}", s);
    }
    println!();

    println!("\n=== System running successfully! ===");
    println!("Memory allocator working correctly!");
    println!("UART output functioning at 0x0900_0000");

    // 进入无限循环
    system_shutdown();
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    println!("PANIC: {}", info);
    loop {
        core::hint::spin_loop();
    }
}

const PSCI_SYSTEM_OFF: usize = 0x84000008;
#[inline]
fn system_shutdown() -> ! {
    println!("Shutting down system...");
    unsafe {
        asm!("hvc #0", in("x0") PSCI_SYSTEM_OFF, in("x1") 0, in("x2") 0, in("x3") 0);
    }

    loop {
        core::hint::spin_loop();
    }
}
