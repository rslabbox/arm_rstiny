//! Untrusted user addresses with explicit access direction.
//! Construction does not validate a mapping or grant access to an address space.
use super::{AddressSpace, Error};
use core::marker::PhantomData;

/// A source address in a user address space, never a kernel reference.
#[repr(transparent)]
pub struct UserConstPtr<T> {
    address: usize,
    pointee: PhantomData<*const T>,
}

/// A destination address in a user address space, never a kernel reference.
#[repr(transparent)]
pub struct UserPtr<T> {
    address: usize,
    pointee: PhantomData<*mut T>,
}

impl<T> From<u64> for UserConstPtr<T> {
    fn from(address: u64) -> Self {
        Self {
            address: address as usize,
            pointee: PhantomData,
        }
    }
}
impl<T> From<u64> for UserPtr<T> {
    fn from(address: u64) -> Self {
        Self {
            address: address as usize,
            pointee: PhantomData,
        }
    }
}

impl UserConstPtr<u8> {
    /// Validate the entire range and copy through the supplied space's owned frames.
    pub fn read(self, space: &AddressSpace, buffer: &mut [u8]) -> Result<(), Error> {
        space.read(self.address, buffer)
    }
}
impl UserPtr<u8> {
    /// Validate the entire writable range before changing any destination byte.
    pub fn write(self, space: &mut AddressSpace, buffer: &[u8]) -> Result<(), Error> {
        space.write(self.address, buffer)
    }
}
