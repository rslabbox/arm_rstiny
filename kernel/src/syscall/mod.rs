//! Userspace ABI boundary, independent of scheduler representation.
mod dispatch;
mod memory;
mod task;
pub(crate) use dispatch::dispatch;

use crate::task::Disposition;
type Result<T> = core::result::Result<T, u64>;
struct Completion {
    value: Option<u64>,
    disposition: Disposition,
}
impl Completion {
    fn done(value: Option<u64>) -> Self {
        Self {
            value,
            disposition: Disposition::Resume,
        }
    }
    fn park(disposition: Disposition) -> Self {
        Self {
            value: None,
            disposition,
        }
    }
}
