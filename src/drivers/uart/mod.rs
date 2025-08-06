mod pl011;
use pl011 as imp;

pub use self::imp::{console_getchar, console_putchar};
pub(super) use self::imp::{init, init_early};
