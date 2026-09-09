//! Dispatch typed system calls and return scheduling decisions.
use super::{Completion, Result, memory, task};
use crate::{
    arch::{irq, user::UserContext},
    task::Disposition,
};
use kernel_abi::*;

pub(crate) fn dispatch(context: &mut UserContext) -> Disposition {
    let Ok(number) = Syscall::try_from(context.syscall_number()) else {
        context.set_syscall_result(Err(UNSUPPORTED));
        return Disposition::Resume;
    };
    let result: Result<Completion> = match number {
        Syscall::Yield => Ok(Completion::done(None)),
        Syscall::DebugPutchar => {
            if context.arg0() > 255 {
                Err(INVALID_ARGUMENT)
            } else if log::max_level() == log::LevelFilter::Off {
                Err(UNSUPPORTED)
            } else {
                crate::utils::logging::debug_putchar(context.arg0() as u8);
                Ok(Completion::done(None))
            }
        }
        Syscall::TaskId => Ok(Completion::done(Some(
            crate::task::current_id().expect("syscall outside user event"),
        ))),
        Syscall::TaskCreate => task::create(),
        Syscall::TaskStart => task::start(
            context.arg0(),
            context.arg1(),
            context.arg2(),
            context.arg3(),
        ),
        Syscall::TaskStatus => task::status(context.arg0()),
        Syscall::TaskSuspend => task::suspend(context.arg0()),
        Syscall::TaskResume => task::resume(context.arg0()),
        Syscall::TaskDestroy => task::destroy(context.arg0()),
        Syscall::Wait => task::wait(context.arg0()),
        Syscall::Sleep => task::sleep(context.arg0()),
        Syscall::SuspendSelf => Ok(Completion::park(Disposition::Suspend)),
        Syscall::Exit => Ok(Completion::park(Disposition::Exit(context.arg0()))),
        Syscall::Clock => Ok(Completion::done(Some(
            irq::now() / (irq::frequency() / 1000).max(1),
        ))),
        Syscall::MemoryAvailable => Ok(Completion::done(Some(
            crate::memory::available_frames() as u64
        ))),
        Syscall::Map => memory::map(
            context.arg0(),
            context.arg1() as usize,
            context.arg2() as usize,
            context.arg3(),
        ),
        Syscall::Unmap => memory::unmap(
            context.arg0(),
            context.arg1() as usize,
            context.arg2() as usize,
        ),
        Syscall::Protect => memory::protect(
            context.arg0(),
            context.arg1() as usize,
            context.arg2() as usize,
            context.arg3(),
        ),
        Syscall::WriteMemory => memory::write(
            context.arg0(),
            context.arg1().into(),
            context.arg2().into(),
            context.arg3() as usize,
        ),
        Syscall::ReadMemory => memory::read(
            context.arg0(),
            context.arg1().into(),
            context.arg2().into(),
            context.arg3() as usize,
        ),
    };
    match result {
        Ok(completion) => {
            // Wait resumes within its handler; exit never returns to userspace.
            if !matches!(completion.disposition, Disposition::Exit(_)) {
                context.set_syscall_result(Ok(completion.value));
            }
            completion.disposition
        }
        Err(code) => {
            context.set_syscall_result(Err(code));
            Disposition::Resume
        }
    }
}
