//! Availability and policy for user debug output; the console backend is separate.
pub(super) fn put_char(byte: u8) -> bool {
    if log::max_level() == log::LevelFilter::Off {
        return false;
    }
    crate::utils::logging::debug_putchar(byte);
    true
}
