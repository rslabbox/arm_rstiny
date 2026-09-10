//! Endpoint IPC from EL0: the six seL4 syscalls plus badge delivery.
//!
//! Messages longer than four registers travel through the task's IPC buffer
//! (TPIDRRO_EL0 runtime convention). Received badges arrive in x0.
use super::{Error, abi};
use core::arch::asm;

/// Message words carried beyond the four register MRs.
pub const MAX_WORDS: usize = 16;

/// Where the kernel lands caps carried by an incoming message: a CNode slot in
/// the receiver's own CSpace. The spec is sticky in the IPC buffer (the kernel
/// never rewrites it), so set it once per landing slot; the slot must be empty
/// at delivery time or the transfer fails (`ALREADY_MAPPED`).
#[derive(Clone, Copy, Debug)]
pub struct ReceiveSpec {
    pub cnode: u64,
    pub index: u64,
    pub depth: u64,
}

/// Publish the landing spec at IPC buffer offset 1000 (kernel ABI).
pub fn set_receive_spec(spec: ReceiveSpec) -> Result<(), Error> {
    let address = ipc_address();
    if address == 0 {
        return Err(Error::TruncatedMessage);
    }
    // SAFETY: the task's runtime owns this IPC buffer; no alias exists.
    unsafe {
        for (offset, word) in [(0usize, spec.cnode), (8, spec.index), (16, spec.depth)] {
            let slot = (address + 1000 + offset) as *mut u64;
            slot.write_volatile(word);
        }
    }
    Ok(())
}

/// One received message: badge, label and the first `length` message registers.
#[derive(Clone, Copy)]
pub struct Received {
    pub badge: u64,
    pub label: u64,
    pub length: usize,
    pub words: [u64; MAX_WORDS],
}
impl Received {
    pub fn word(&self, index: usize) -> u64 {
        self.words.get(index).copied().unwrap_or(0)
    }
}

fn ipc_address() -> usize {
    let address: usize;
    // Kernel runtime convention; the register is read-only at EL0.
    unsafe {
        core::arch::asm!("mrs {address}, tpidrro_el0", address = out(reg) address, options(nomem, nostack))
    };
    address
}

fn marshal(words: &[u64], caps: &[u64]) -> Result<(), Error> {
    if words.len() > MAX_WORDS {
        return Err(Error::TruncatedMessage);
    }
    if words.len() > 4 || !caps.is_empty() {
        let address = ipc_address();
        if address == 0 {
            return Err(Error::TruncatedMessage);
        }
        // SAFETY: the task's runtime owns this IPC buffer; no alias exists.
        unsafe {
            for index in 4..words.len() {
                let slot = (address + 8 + index * 8) as *mut u64;
                slot.write_volatile(words[index]);
            }
            for (index, cap) in caps.iter().enumerate() {
                let slot = (address + 976 + index * 8) as *mut u64;
                slot.write_volatile(*cap);
            }
        }
    }
    Ok(())
}

fn unmarshal(badge: u64, tag: u64, registers: [u64; 4]) -> Received {
    let info = abi::MessageInfo::from_word(tag);
    let mut received = Received {
        badge,
        label: info.label(),
        length: info.length().min(MAX_WORDS),
        words: [0; MAX_WORDS],
    };
    received.words[..4].copy_from_slice(&registers);
    if received.length > 4 {
        let address = ipc_address();
        // SAFETY: a received message implies the kernel wrote this buffer.
        unsafe {
            for index in 4..received.length {
                let slot = (address + 8 + index * 8) as *const u64;
                received.words[index] = slot.read_volatile();
            }
        }
    }
    received
}

fn syscall(
    number: i64,
    cap: u64,
    label: u64,
    words: &[u64],
    caps: &[u64],
) -> Result<Received, Error> {
    if caps.len() > abi::MAX_EXTRA_CAPS {
        return Err(Error::TruncatedMessage);
    }
    marshal(words, caps)?;
    let mut tag = abi::MessageInfo::new(label, caps.len(), words.len()).word();
    let mut badge_or_cap = cap;
    let mut mr0 = words.first().copied().unwrap_or(0);
    let mut mr1 = words.get(1).copied().unwrap_or(0);
    let mut mr2 = words.get(2).copied().unwrap_or(0);
    let mut mr3 = words.get(3).copied().unwrap_or(0);
    unsafe {
        asm!(
            "svc #0",
            in("x7") number as u64,
            inlateout("x0") badge_or_cap => badge_or_cap,
            inlateout("x1") tag,
            inlateout("x2") mr0,
            inlateout("x3") mr1,
            inlateout("x4") mr2,
            inlateout("x5") mr3,
        );
    }
    Ok(unmarshal(badge_or_cap, tag, [mr0, mr1, mr2, mr3]))
}

/// Buffered send with no receiver wait. `caps` transfer Grant-authorised caps.
pub fn send(cap: u64, label: u64, words: &[u64]) -> Result<Received, Error> {
    syscall(abi::Syscall::Send as i64, cap, label, words, &[])
}

/// Buffered non-blocking send.
pub fn nbsend(cap: u64, label: u64, words: &[u64]) -> Result<Received, Error> {
    syscall(abi::Syscall::NBSend as i64, cap, label, words, &[])
}

/// Buffered call: send, block for the reply, return the reply message.
pub fn call(cap: u64, label: u64, words: &[u64]) -> Result<Received, Error> {
    syscall(abi::Syscall::Call as i64, cap, label, words, &[])
}

/// Call that transfers Grant-authorised caps (client-provided resources).
pub fn call_cap(cap: u64, label: u64, words: &[u64], caps: &[u64]) -> Result<Received, Error> {
    syscall(abi::Syscall::Call as i64, cap, label, words, caps)
}

/// Buffered receive; blocks until a sender or fault arrives.
pub fn recv(cap: u64) -> Result<Received, Error> {
    syscall(abi::Syscall::Recv as i64, cap, 0, &[], &[])
}

/// Buffered non-blocking receive: `None` when nothing is pending.
pub fn nbrecv(cap: u64) -> Result<Option<Received>, Error> {
    let received = syscall(abi::Syscall::NBRecv as i64, cap, 0, &[], &[])?;
    Ok((received.badge != 0 || received.length != 0).then_some(received))
}

/// Reply to the caller or faulted thread recorded by the last delivery.
pub fn reply(label: u64, words: &[u64]) -> Result<(), Error> {
    syscall(abi::Syscall::Reply as i64, 0, label, words, &[]).map(|_| ())
}

/// Reply that transfers Grant-authorised caps (server-provided resources,
/// e.g. a shared buffer granted during a BIND handshake).
pub fn reply_cap(label: u64, words: &[u64], caps: &[u64]) -> Result<(), Error> {
    syscall(abi::Syscall::Reply as i64, 0, label, words, caps).map(|_| ())
}

/// Reply to the pending partner, then receive the next message.
pub fn reply_recv(cap: u64, label: u64, words: &[u64]) -> Result<Received, Error> {
    syscall(abi::Syscall::ReplyRecv as i64, cap, label, words, &[])
}

/// Consume the pending reply relationship by failing it: a supervisor that
/// decides not to answer must still not strand the caller.
pub fn drop_reply() -> Result<(), Error> {
    syscall(abi::Syscall::Reply as i64, 0, 0, &[0], &[])
        .map(|_| ())
        .or(Ok(()))
}
