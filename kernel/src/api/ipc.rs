//! Endpoint, notification and reply IPC: the seL4 non-MCS slow path.
//!
//! Every operation validates completely before it commits, and a blocked task
//! is only ever woken by a waker that also wrote its registers, restart PC or
//! error. Continuations parked in a blocking syscall read back a completion:
//! `Some(1)` proceed, `Some(0)` the partner is gone (the call fails), `None`
//! suspended and resumed (the wait repeats). The running task's own
//! `Task.execution` is checked out while it executes, so any message leg that
//! involves the current task reads or writes the live user context instead of
//! a saved frame.
use super::message;
use crate::{
    arch::kernel::thread::user::UserContext,
    object::{self, ObjectId, ObjectKind},
    task::{self, Blocked, Caller, Disposition, FaultMsg, api},
};
use kernel_abi::*;

fn current() -> u64 {
    task::current_id().expect("IPC syscall outside a task")
}

fn ok() -> Disposition {
    Disposition::Resume
}

/// Fail a syscall with an error label, keeping the kernel reply convention.
fn fail(context: &mut UserContext, error: u64) -> Disposition {
    message::reply(context, error, &[0]);
    ok()
}

/// What happens to the sender once its message is delivered.
#[derive(Clone, Copy)]
enum SenderFate {
    /// Plain send or reply: the sender continues.
    Wake,
    /// Call sender: parked until this receiver replies.
    ReplyWait,
    /// Faulted thread: parked until its supervisor replies; the message is
    /// the recorded fault, not the sender's registers.
    FaultWait,
}

/// Everything a delivery grants: badge, reply right and sender fate.
struct Delivery {
    badge: u64,
    caller: Option<Caller>,
    sender: SenderFate,
}

/// The oldest queue entry whose task is still blocked on this object and
/// whose role matches `states`. Genuinely stale entries (task gone, or no
/// longer waiting on this object) are pruned on the way past; live entries in
/// another role — e.g. a queued fault sender when a sender scans for a
/// receiver — stay queued for the peer that can consume them.
fn peek_valid(ep: ObjectId, states: &[u64]) -> Result<Option<u64>, u64> {
    loop {
        let Some(id) = object::with_wait_queue(ep, |queue| queue.peek())? else {
            return Ok(None);
        };
        let stale = stale_entry(id, ep);
        if stale {
            object::with_wait_queue(ep, |queue| queue.pop())?;
            continue;
        }
        let state = api::state_of(id)?;
        return Ok(states.contains(&state).then_some(id));
    }
}

/// A queue entry is stale when its task is gone or no longer waiting on this
/// object. Entries in another wait role — a queued fault sender ahead of a
/// plain call, say — are live and belong to a different consumer.
fn stale_entry(entry: u64, ep: ObjectId) -> bool {
    api::state_of(entry).is_err() || !api::blocked_of(entry).is_some_and(|blocked| blocked.ep == ep)
}

/// Make room and enqueue `id`, pruning entries that no longer name a task
/// waiting here. The caller commits its own `Blocked` state; enqueue never
/// overwrites it (a queued fault sender keeps its fault marker).
fn enqueue(ep: ObjectId, id: u64, _state: u64) -> Result<(), u64> {
    object::with_wait_queue(ep, |queue| queue.retain(|entry| !stale_entry(entry, ep)))?;
    object::with_wait_queue(ep, |queue| queue.push(id))??;
    Ok(())
}

fn set_recv_blocked(id: u64, ep: ObjectId) {
    api::set_blocked(
        id,
        Blocked {
            ep,
            badge: 0,
            call: false,
            grant_reply: false,
            fault: false,
        },
        TASK_BLOCKED_RECV,
    );
}

/// Assemble one outgoing message from a sender's live context, saved frame or
/// recorded fault.
/// One outgoing message: label, length, extra-cap count and payload registers.
struct Assembled {
    label: u64,
    length: usize,
    extra_caps: usize,
    words: [u64; MAX_MESSAGE_WORDS],
    caps: [u64; MAX_EXTRA_CAPS],
}

/// Assemble one outgoing message from a sender's live context, saved frame or
/// recorded fault.
fn assemble(
    context: Option<&mut UserContext>,
    sender: u64,
    fate: SenderFate,
) -> Result<Assembled, u64> {
    if matches!(fate, SenderFate::FaultWait) {
        let fault = api::fault_msg_of(sender).ok_or(INVALID_CAPABILITY)?;
        let mut words = [0; MAX_MESSAGE_WORDS];
        words[..4].copy_from_slice(&fault.mrs);
        return Ok(Assembled {
            label: fault.label,
            length: fault.length,
            extra_caps: 0,
            words,
            caps: [0; MAX_EXTRA_CAPS],
        });
    }
    let (tag, registers) = match context {
        Some(context) => {
            let registers = [
                context.message_register(0),
                context.message_register(1),
                context.message_register(2),
                context.message_register(3),
            ];
            (context.message_info(), registers)
        }
        None => api::edit_frame(sender, |frame| {
            let registers = [
                frame.message_register(0),
                frame.message_register(1),
                frame.message_register(2),
                frame.message_register(3),
            ];
            (frame.message_info(), registers)
        })?,
    };
    let info = MessageInfo::from_word(tag);
    let mut words = [0; MAX_MESSAGE_WORDS];
    words[..4].copy_from_slice(&registers);
    if info.length() > 4 {
        let mut bytes = [0u8; 8 * MAX_MESSAGE_WORDS];
        api::read_task_ipc(sender, 8 + 4 * 8, &mut bytes[..(info.length() - 4) * 8])?;
        for (index, word) in words[4..info.length()].iter_mut().enumerate() {
            *word = u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap());
        }
    }
    let mut caps = [0; MAX_EXTRA_CAPS];
    if info.extra_caps() > 0 {
        let mut bytes = [0u8; 8 * MAX_EXTRA_CAPS];
        api::read_task_ipc(sender, 976, &mut bytes[..info.extra_caps() * 8])?;
        for (index, cap) in caps.iter_mut().enumerate().take(info.extra_caps()) {
            *cap = u64::from_le_bytes(bytes[index * 8..index * 8 + 8].try_into().unwrap());
        }
    }
    Ok(Assembled {
        label: info.label(),
        length: info.length(),
        extra_caps: info.extra_caps(),
        words,
        caps,
    })
}

/// Deliver one message into `receiver` on behalf of `sender`. All validation
/// — buffers, receive spec, cap rights, landing slots — happens before the
/// first write, so a failed delivery changes nothing. The context arguments
/// supply the live user frame whenever that side is the running task.
fn deliver(
    receiver: u64,
    sender: u64,
    delivery: Delivery,
    sender_context: Option<&mut UserContext>,
    receiver_context: Option<&mut UserContext>,
) -> Result<(), u64> {
    let me = current();
    let msg = assemble(sender_context, sender, delivery.sender)?;
    let (label, length, extra_caps, words, caps) =
        (msg.label, msg.length, msg.extra_caps, msg.words, msg.caps);
    // Validate the receiver side completely before any commit.
    if length > 4 || extra_caps > 0 {
        api::check_task_ipc(receiver, 8, length * 8)?;
        if extra_caps > 0 {
            api::check_task_ipc(receiver, 976, extra_caps * 8)?;
            api::check_task_ipc(receiver, 1000, 24)?;
        }
    }
    let mut landing: Option<(ObjectId, u64)> = None;
    if extra_caps > 0 {
        let mut spec = [0u8; 24];
        api::read_task_ipc(receiver, 1000, &mut spec)?;
        let cnode_slot = u64::from_le_bytes(spec[0..8].try_into().unwrap());
        let index = u64::from_le_bytes(spec[8..16].try_into().unwrap());
        let depth = u64::from_le_bytes(spec[16..24].try_into().unwrap());
        if depth != 64 || index == 0 || index + extra_caps as u64 > CNODE_SLOTS {
            return Err(INVALID_ARGUMENT);
        }
        let (kind, node, _, rights) = object::lookup_in(api::cspace_of(receiver)?, cnode_slot)?;
        if kind != ObjectKind::CNode || rights & RIGHTS_GRANT == 0 {
            return Err(INVALID_CAPABILITY);
        }
        for offset in 0..extra_caps as u64 {
            if !object::slot_empty(node, index + offset)? {
                return Err(ALREADY_MAPPED);
            }
        }
        landing = Some((node, index));
    }
    // Commit: buffer words, cap badges, cap transfer, registers, bookkeeping.
    if length > 4 {
        let mut bytes = [0u8; 8 * MAX_MESSAGE_WORDS];
        for (index, word) in words.iter().take(length).enumerate() {
            bytes[index * 8..index * 8 + 8].copy_from_slice(&word.to_le_bytes());
        }
        api::write_task_ipc(receiver, 8, &bytes[..length * 8])?;
    }
    if let Some((node, index)) = landing {
        // Received caps carry no badges on the wire; unwrap is not implemented.
        api::write_task_ipc(receiver, 976, &[0u8; 8 * MAX_EXTRA_CAPS][..extra_caps * 8])?;
        object::transfer_caps(api::cspace_of(sender)?, &caps[..extra_caps], node, index)?;
    }
    let tag = MessageInfo::new(label, extra_caps, length).word();
    let head = &words[..length.min(4)];
    match receiver_context {
        Some(context) => context.set_reply(delivery.badge, tag, head),
        None => api::edit_frame(receiver, |frame| frame.set_reply(delivery.badge, tag, head))?,
    }
    if let Some(caller) = delivery.caller
        && let Some(displaced) = api::set_caller(receiver, caller)
    {
        api::fail_caller(displaced);
    }
    if receiver != me {
        api::complete(receiver)?;
    }
    match delivery.sender {
        SenderFate::Wake if sender == me => {}
        SenderFate::Wake => api::complete(sender)?,
        // A caller stays parked until the reply: no intermediate wakeup, so
        // its parked state is never clobbered by scheduling.
        SenderFate::ReplyWait => {
            api::clear_blocked(sender);
            api::set_state(sender, TASK_BLOCKED_REPLY);
        }
        SenderFate::FaultWait => api::clear_blocked(sender),
    }
    Ok(())
}

/// Block once; the return value is the completion written by the waker.
fn wait() -> Option<u64> {
    task::park(Disposition::Block)
}

/// Park the current task until a reply arrives in its registers.
fn wait_reply(context: &mut UserContext) -> Disposition {
    loop {
        match wait() {
            Some(1) => return ok(),
            Some(_) => return fail(context, UNSUPPORTED),
            None => {
                // Delivered as a call (now awaiting the reply) or resumed after
                // a suspension: either way, keep waiting for the reply.
                api::set_state(current(), TASK_BLOCKED_REPLY);
            }
        }
    }
}

fn blocking_send(
    context: &mut UserContext,
    ep: ObjectId,
    badge: u64,
    grant_reply: bool,
    call: bool,
) -> Result<Disposition, u64> {
    let me = current();
    loop {
        if let Some(receiver) = peek_valid(ep, &[TASK_BLOCKED_RECV])? {
            let delivery = Delivery {
                badge,
                caller: (call && grant_reply).then_some(Caller::Call(me)),
                sender: if call {
                    SenderFate::ReplyWait
                } else {
                    SenderFate::Wake
                },
            };
            deliver(receiver, me, delivery, Some(context), None)?;
            object::with_wait_queue(ep, |queue| queue.pop())?;
            return Ok(if call { wait_reply(context) } else { ok() });
        }
        enqueue(ep, me, TASK_BLOCKED_SEND)?;
        api::set_blocked(
            me,
            Blocked {
                ep,
                badge,
                call,
                grant_reply,
                fault: false,
            },
            TASK_BLOCKED_SEND,
        );
        match wait() {
            Some(1) => return Ok(ok()),
            Some(_) => return Ok(fail(context, UNSUPPORTED)),
            // Delivered as a call (now awaiting the reply) or resumed after a
            // suspension (re-queue): re-evaluate from the top.
            None => {
                if api::state_of(me)? == TASK_BLOCKED_REPLY {
                    return Ok(wait_reply(context));
                }
            }
        }
    }
}

fn blocking_receive(
    context: &mut UserContext,
    ep: ObjectId,
    non_blocking: bool,
) -> Result<Disposition, u64> {
    let me = current();
    loop {
        if let Some(sender) = peek_valid(ep, &[TASK_BLOCKED_SEND, TASK_BLOCKED_FAULT])? {
            let blocked = api::blocked_of(sender).ok_or(INVALID_CAPABILITY)?;
            let delivery = if blocked.fault {
                Delivery {
                    badge: blocked.badge,
                    caller: Some(Caller::Fault(sender)),
                    sender: SenderFate::FaultWait,
                }
            } else {
                Delivery {
                    badge: blocked.badge,
                    caller: (blocked.call && blocked.grant_reply).then_some(Caller::Call(sender)),
                    sender: if blocked.call {
                        SenderFate::ReplyWait
                    } else {
                        SenderFate::Wake
                    },
                }
            };
            deliver(me, sender, delivery, None, Some(context))?;
            object::with_wait_queue(ep, |queue| queue.pop())?;
            return Ok(ok());
        }
        if non_blocking {
            // seL4 NBRecv with nothing pending: badge 0, empty tag.
            context.set_reply(0, 0, &[]);
            return Ok(ok());
        }
        enqueue(ep, me, TASK_BLOCKED_RECV)?;
        set_recv_blocked(me, ep);
        match wait() {
            Some(1) => return Ok(ok()),
            Some(_) => return Ok(fail(context, UNSUPPORTED)),
            None => continue,
        }
    }
}

/// Consume the current task's reply relation. `Ok(false)` means there was
/// none: an error for `Reply`, silently skipped by `ReplyRecv` (seL4 allows a
/// null caller slot there).
fn reply_phase(context: &mut UserContext) -> Result<bool, u64> {
    let me = current();
    let Some(caller) = api::take_caller(me) else {
        return Ok(false);
    };
    let target = caller.task();
    let state = api::state_of(target)?;
    let suspended = state == TASK_SUSPENDED
        && matches!(
            api::suspended_from_of(target),
            TASK_BLOCKED_REPLY | TASK_BLOCKED_FAULT
        );
    match caller {
        Caller::Fault(_) => {
            if state != TASK_BLOCKED_FAULT && !suspended {
                return Ok(true);
            }
            // Restart the faulted thread at its saved PC; a supervisor that
            // needed a register fix used TCB_WriteRegisters beforehand.
            let pc = api::restart_pc_of(target);
            api::edit_frame(target, |frame| frame.frame_mut().elr = pc)?;
            api::complete(target)?;
        }
        Caller::Call(_) => {
            if state != TASK_BLOCKED_REPLY && !suspended {
                return Ok(true);
            }
            let delivery = Delivery {
                badge: 0,
                caller: None,
                sender: SenderFate::Wake,
            };
            // A failed reply delivery (no receive spec, occupied landing slot,
            // missing grant) must still wake the parked caller with a failed
            // completion: its relation is consumed, so nothing else ever will.
            if let Err(error) = deliver(target, me, delivery, Some(context), None) {
                api::fail_caller(caller);
                return Err(error);
            }
        }
    }
    Ok(true)
}

/// Signal a notification: wake one waiter with the badge, or merge into bits.
fn signal(ntfn: ObjectId, badge: u64) -> Result<(), u64> {
    if let Some(waiter) = peek_valid(ntfn, &[TASK_BLOCKED_RECV])? {
        api::edit_frame(waiter, |frame| frame.set_reply(badge, 0, &[]))?;
        api::complete(waiter)?;
        object::with_wait_queue(ntfn, |queue| queue.pop())?;
    } else {
        object::with_notification_bits(ntfn, |bits| *bits |= badge)?;
    }
    Ok(())
}

fn wait_notification(
    context: &mut UserContext,
    ntfn: ObjectId,
    non_blocking: bool,
) -> Result<Disposition, u64> {
    let me = current();
    loop {
        let bits = object::with_notification_bits(ntfn, |bits| core::mem::replace(bits, 0))?;
        if bits != 0 {
            context.set_reply(bits, 0, &[]);
            return Ok(ok());
        }
        if non_blocking {
            context.set_reply(0, 0, &[]);
            return Ok(ok());
        }
        enqueue(ntfn, me, TASK_BLOCKED_RECV)?;
        set_recv_blocked(me, ntfn);
        match wait() {
            Some(1) => return Ok(ok()),
            Some(_) => return Ok(fail(context, UNSUPPORTED)),
            None => continue,
        }
    }
}

fn run(number: Syscall, context: &mut UserContext) -> Result<Disposition, u64> {
    if matches!(number, Syscall::Reply | Syscall::ReplyRecv) {
        let replied = reply_phase(context)?;
        if number == Syscall::Reply {
            if !replied {
                return Err(INVALID_CAPABILITY);
            }
            return Ok(ok());
        }
    }
    let (kind, id, badge, rights) = object::lookup(context.arg0())?;
    match number {
        Syscall::Send | Syscall::NBSend | Syscall::Call => match kind {
            ObjectKind::Endpoint => {
                if rights & RIGHTS_WRITE == 0 {
                    return Err(PERMISSION_DENIED);
                }
                if number == Syscall::NBSend {
                    // Deliver only to a receiver that already waits.
                    if let Some(receiver) = peek_valid(id, &[TASK_BLOCKED_RECV])? {
                        let delivery = Delivery {
                            badge,
                            caller: None,
                            sender: SenderFate::Wake,
                        };
                        deliver(receiver, current(), delivery, Some(context), None)?;
                        object::with_wait_queue(id, |queue| queue.pop())?;
                    }
                    return Ok(ok());
                }
                blocking_send(
                    context,
                    id,
                    badge,
                    rights & RIGHTS_GRANT_REPLY != 0,
                    number == Syscall::Call,
                )
            }
            ObjectKind::Notification => {
                if number == Syscall::Call {
                    return Err(UNSUPPORTED);
                }
                if rights & RIGHTS_WRITE == 0 {
                    return Err(PERMISSION_DENIED);
                }
                signal(id, badge)?;
                Ok(ok())
            }
            _ => Err(INVALID_CAPABILITY),
        },
        Syscall::Recv | Syscall::NBRecv | Syscall::ReplyRecv => match kind {
            ObjectKind::Endpoint => {
                if rights & RIGHTS_READ == 0 {
                    return Err(PERMISSION_DENIED);
                }
                blocking_receive(context, id, number == Syscall::NBRecv)
            }
            ObjectKind::Notification => {
                if rights & RIGHTS_READ == 0 {
                    return Err(PERMISSION_DENIED);
                }
                wait_notification(context, id, number == Syscall::NBRecv)
            }
            _ => Err(INVALID_CAPABILITY),
        },
        _ => Err(UNSUPPORTED),
    }
}

/// Syscall entry. Object calls on endpoint capabilities are routed here too.
pub(crate) fn syscall(number: Syscall, context: &mut UserContext) -> Disposition {
    match run(number, context) {
        Ok(disposition) => disposition,
        Err(error) => fail(context, error),
    }
}

/// Deliver a fault from `id` through its fault endpoint and park until the
/// supervisor replies. `Ok` means the thread may resume at its restart PC.
pub(crate) fn send_fault(
    id: u64,
    slot: u64,
    label: u64,
    length: usize,
    mrs: [u64; 4],
    restart_pc: u64,
) -> Result<(), u64> {
    let (kind, ep, badge, _) = object::lookup(slot)?;
    if kind != ObjectKind::Endpoint {
        return Err(INVALID_CAPABILITY);
    }
    api::begin_fault(id, ep, badge, FaultMsg { label, length, mrs }, restart_pc);
    if let Some(receiver) = peek_valid(ep, &[TASK_BLOCKED_RECV])? {
        let delivery = Delivery {
            badge,
            caller: Some(Caller::Fault(id)),
            sender: SenderFate::FaultWait,
        };
        deliver(receiver, id, delivery, None, None)?;
        object::with_wait_queue(ep, |queue| queue.pop())?;
    } else {
        enqueue(ep, id, TASK_BLOCKED_FAULT)?;
    }
    loop {
        match task::park(Disposition::Block) {
            Some(1) => return Ok(()),
            Some(_) => return Err(INVALID_CAPABILITY),
            None => continue,
        }
    }
}
