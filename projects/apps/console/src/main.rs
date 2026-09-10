#![no_std]
#![no_main]
use rstiny_protocol::{Argument, console, control};
use rstiny_runtime::entry;
use rstiny_server::Service;

const CONSOLE_VA: usize = 0x0300_0000;
const UART_UNTYPED_SLOT: usize = 1; // SpawnInfo::extra[1] (device); extra[2] = ordinary budget
/// Deliberate crash trigger for the restart-acceptance scenario: a write of
/// this length faults the service so the supervisor's restart path runs.
const CRASH_MAGIC: usize = 0xCAFE_BABE;

const DR: usize = 0x000;
const FR: usize = 0x018;
const IBRD: usize = 0x024;
const FBRD: usize = 0x028;
const LCR: usize = 0x02C;
const CR: usize = 0x030;
const IFLS: usize = 0x038;

fn uart_init(base: usize) {
    let write = |register: usize, value: u32| unsafe {
        core::ptr::write_volatile((base + register) as *mut u32, value);
    };
    write(CR, 0);
    write(IFLS, 0);
    // 24 MHz clock, 115200 baud, 8N1.
    write(IBRD, 13);
    write(FBRD, 1);
    write(LCR, 0b11 << 5 | 0b11);
    write(CR, 1 << 0 | 1 << 8 | 1 << 9);
}

fn putc(base: usize, byte: u8) {
    // SAFETY: the UART page is a device frame exclusively owned by this task.
    unsafe {
        while core::ptr::read_volatile((base + FR) as *const u32) & (1 << 5) != 0 {
            core::hint::spin_loop();
        }
        core::ptr::write_volatile((base + DR) as *mut u8, byte);
    }
}

fn flush(base: usize) {
    // SAFETY: as above.
    unsafe {
        while core::ptr::read_volatile((base + FR) as *const u32) & (1 << 3) != 0 {
            core::hint::spin_loop();
        }
    }
}

#[entry]
fn main(argument: Argument) -> ! {
    let Some(service) = Service::init(argument) else {
        loop {
            core::hint::spin_loop();
        }
    };
    let Some(untyped) = service
        .extra
        .get(UART_UNTYPED_SLOT)
        .copied()
        .filter(|s| *s != 0)
    else {
        service.exit(2);
    };
    // The UART device frame comes from the device Untyped; the covering L3
    // comes from the service's ordinary budget (device memory stays a frame).
    use rstiny::capability::{CNode, CPtr, ObjectType, Page, PageTable, Untyped, VM_EXECUTE_NEVER};
    let cnode = CNode(CPtr(rstiny::capability::INIT_CNODE));
    let page_slot = 40u64;
    let table_slot = 41u64;
    Untyped(CPtr(untyped))
        .retype(ObjectType::SmallPage, 0, cnode.0, page_slot, 1)
        .unwrap();
    // The covering L3 comes from the service's own budget (slot 32).
    Untyped(CPtr(rstiny::capability::INIT_UNTYPED))
        .retype(ObjectType::PageTable, 0, cnode.0, table_slot, 1)
        .unwrap();
    unsafe {
        PageTable(CPtr(table_slot))
            .map(
                CPtr(rstiny::capability::INIT_VSPACE),
                CONSOLE_VA & !0x1F_FFFF,
            )
            .unwrap();
        Page(CPtr(page_slot))
            .map(
                CPtr(rstiny::capability::INIT_VSPACE),
                CONSOLE_VA,
                rstiny::capability::RIGHTS_READ | rstiny::capability::RIGHTS_WRITE,
                VM_EXECUTE_NEVER,
            )
            .unwrap();
    }
    uart_init(CONSOLE_VA);
    // The console service is its own output device: the banner goes directly
    // to the UART (a console client must never log through itself).
    for byte in "[console] console service ready\n".bytes() {
        putc(CONSOLE_VA, byte);
    }
    flush(CONSOLE_VA);

    loop {
        if service.poll_stop() {
            flush(CONSOLE_VA);
            service.exit(0);
        }
        let Ok(received) = rstiny::ipc::recv(service.console_ep) else {
            continue;
        };
        match received.label {
            console::WRITE => {
                let count = received.word(0) as usize;
                if count == CRASH_MAGIC {
                    // Acceptance hook: a controlled fault exercises the
                    // supervisor's restart path end to end.
                    for byte in "[console] crash requested; faulting now\n".bytes() {
                        putc(CONSOLE_VA, byte);
                    }
                    // SAFETY: deliberately invalid; this is the crash trigger.
                    unsafe { core::ptr::write_volatile(0usize as *mut u64, 1) };
                }
                if count > console::MAX_WRITE || count > (received.length - 1) * 8 {
                    let _ = rstiny::ipc::reply(1, &[0]);
                    continue;
                }
                for index in 0..count {
                    let word = received.word(1 + index / 8);
                    putc(CONSOLE_VA, (word >> (8 * (index % 8))) as u8);
                }
                flush(CONSOLE_VA);
                let _ = rstiny::ipc::reply(0, &[count as u64]);
            }
            console::BIND => {
                let _ = rstiny::ipc::reply(0, &[1, console::MAX_WRITE as u64]);
            }
            control::STOP => {
                // Graceful stop: flush, acknowledge, exit. The supervisor
                // tears the task down if the ack never arrives.
                flush(CONSOLE_VA);
                let _ = rstiny::ipc::reply(control::STOP_ACK, &[]);
                service.exit(0);
            }
            _ => {
                let _ = rstiny::ipc::reply(1, &[]);
            }
        }
    }
}
