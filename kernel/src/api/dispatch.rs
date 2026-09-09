//! Dispatch seL4 calls and object invocations through the architecture ABI accessors.
use super::{
    Completion,
    message::{reply, request},
};
use crate::{arch::kernel::thread::user::UserContext, task::Disposition};
use kernel_abi::*;

pub(crate) fn dispatch(context: &mut UserContext) -> Disposition {
    let number = match Syscall::try_from(context.syscall_number()) {
        Ok(number) => number,
        // No fault endpoint exists yet; unknown calls terminate only this task.
        Err(_) => return Disposition::Fault(context.syscall_number()),
    };
    match number {
        Syscall::Yield => Disposition::Resume,
        Syscall::DebugPutchar => {
            if !super::debug::put_char(context.arg0() as u8) {
                return Disposition::Fault(context.syscall_number());
            }
            Disposition::Resume
        }
        Syscall::Call => {
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
        // Endpoint IPC is not implemented. Never pretend an object invocation
        // or kernel runtime service is a user endpoint send/receive.
        Syscall::ReplyRecv
        | Syscall::Send
        | Syscall::NBSend
        | Syscall::Recv
        | Syscall::Reply
        | Syscall::NBRecv => Disposition::Fault(context.syscall_number()),
    }
}
