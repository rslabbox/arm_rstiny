//! Task handlers translate requests into authorized operations or explicit waits.
use super::{Completion, Result};
use crate::{
    arch::irq,
    task::{Disposition, api},
};

pub(super) fn create() -> Result<Completion> {
    Ok(Completion::done(Some(api::create()?)))
}
pub(super) fn start(target: u64, entry: u64, stack: u64, argument: u64) -> Result<Completion> {
    api::start(target, entry, stack, argument, super::dispatch)?;
    Ok(Completion::done(None))
}
pub(super) fn suspend(target: u64) -> Result<Completion> {
    api::suspend(target)?;
    Ok(Completion::done(None))
}
pub(super) fn resume(target: u64) -> Result<Completion> {
    api::resume(target)?;
    Ok(Completion::done(None))
}
pub(super) fn destroy(target: u64) -> Result<Completion> {
    api::destroy(target)?;
    Ok(Completion::done(None))
}
pub(super) fn status(target: u64) -> Result<Completion> {
    Ok(Completion::done(Some(api::status(target)?)))
}
pub(super) fn wait(target: u64) -> Result<Completion> {
    Ok(match api::wait_result(target)? {
        Some(value) => Completion::done(Some(value)),
        None => {
            // This task's kernel continuation resumes here after child exit.
            let result = crate::task::park(Disposition::Wait(target))
                .expect("wait resumed without completion");
            Completion::done(Some(result))
        }
    })
}
pub(super) fn sleep(milliseconds: u64) -> Result<Completion> {
    let duration = milliseconds
        .checked_mul(irq::frequency())
        .ok_or(kernel_abi::INVALID_ARGUMENT)?
        / 1000;
    let deadline = irq::now()
        .checked_add(duration)
        .ok_or(kernel_abi::INVALID_ARGUMENT)?;
    Ok(Completion::park(Disposition::Sleep(deadline)))
}
