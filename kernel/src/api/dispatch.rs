//! Dispatch seL4 calls and object invocations through the architecture ABI accessors.
use super::{
    Completion, ipc,
    message::{reply, request},
};
use crate::{arch::kernel::thread::user::UserContext, object::ObjectKind, task::Disposition};
use kernel_abi::*;

pub(crate) fn dispatch(context: &mut UserContext) -> Disposition {
    let number = match Syscall::try_from(context.syscall_number()) {
        Ok(number) => number,
        // An unknown call is a fault like any other: it reaches the task's
        // fault endpoint when one is configured.
        Err(_) => return super::faults::unknown_syscall(context.frame(), context.syscall_number()),
    };
    match number {
        Syscall::Yield => Disposition::Resume,
        Syscall::DebugPutchar => {
            if !super::debug::put_char(context.arg0() as u8) {
                return super::faults::unknown_syscall(context.frame(), context.syscall_number());
            }
            Disposition::Resume
        }
        Syscall::Call => {
            // Calls on endpoint or notification capabilities are IPC; every
            // other capability is an object invocation.
            let ipc_target = matches!(
                crate::object::lookup(context.arg0()),
                Ok((ObjectKind::Endpoint | ObjectKind::Notification, ..))
            );
            if ipc_target {
                return ipc::syscall(number, context);
            }
            let result =
                request(context).and_then(|message| crate::object::call(context.arg0(), &message));
            match result {
                Ok(Completion { value, disposition }) => {
                    if !matches!(disposition, Disposition::Exit(_)) {
                        match value {
                            Some(value) => reply(context, OK, &[value]),
                            None => reply(context, OK, &[]),
                        }
                    }
                    disposition
                }
                Err(error) => {
                    reply(context, error, &[0]);
                    Disposition::Resume
                }
            }
        }
        // Endpoint IPC.
        Syscall::ReplyRecv
        | Syscall::Send
        | Syscall::NBSend
        | Syscall::Recv
        | Syscall::Reply
        | Syscall::NBRecv => ipc::syscall(number, context),
    }
}
