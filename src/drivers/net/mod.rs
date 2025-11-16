//! Network device drivers

pub mod arp;
pub mod rtl8125;
pub mod netstack;

pub use rtl8125::Rtl8125;
pub use netstack::test_ping;
