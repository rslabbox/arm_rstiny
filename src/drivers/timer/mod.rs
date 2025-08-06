mod arm_generic_timer;
pub(super) use self::imp::init;
pub use self::imp::{current_ticks, nanos_to_ticks, set_oneshot_timer, ticks_to_nanos};
use arm_generic_timer as imp;
