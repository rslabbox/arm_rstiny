//! Memory syscall bounds and direction; address-space ownership stays in task API.
use super::{Completion, Result};
use crate::{
    memory::{UserConstPtr, UserPtr},
    task::api,
};
const MAX_COPY: usize = 4096;

pub(super) fn map(target: u64, address: usize, length: usize, rights: u64) -> Result<Completion> {
    api::edit_space(target, |space| space.map(address, length, rights, false))?;
    Ok(Completion::done(None))
}
pub(super) fn unmap(target: u64, address: usize, length: usize) -> Result<Completion> {
    api::edit_space(target, |space| space.unmap(address, length))?;
    Ok(Completion::done(None))
}
pub(super) fn protect(
    target: u64,
    address: usize,
    length: usize,
    rights: u64,
) -> Result<Completion> {
    api::edit_space(target, |space| space.protect(address, length, rights))?;
    Ok(Completion::done(None))
}
pub(super) fn write(
    target: u64,
    destination: UserPtr<u8>,
    source: UserConstPtr<u8>,
    length: usize,
) -> Result<Completion> {
    if length > MAX_COPY {
        return Err(kernel_abi::INVALID_ARGUMENT);
    }
    let mut buffer = [0; MAX_COPY];
    api::write_memory(target, destination, source, &mut buffer[..length])?;
    Ok(Completion::done(None))
}
pub(super) fn read(
    target: u64,
    source: UserConstPtr<u8>,
    destination: UserPtr<u8>,
    length: usize,
) -> Result<Completion> {
    if length > MAX_COPY {
        return Err(kernel_abi::INVALID_ARGUMENT);
    }
    let mut buffer = [0; MAX_COPY];
    api::read_memory(target, source, destination, &mut buffer[..length])?;
    Ok(Completion::done(None))
}
