//! Standard log facade, configured by LOG at build time (default: info).
use core::{
    fmt::{self, Display, Write},
    sync::atomic::{AtomicBool, Ordering},
};
use log::{Level, LevelFilter, Log, Metadata, Record};

static WRITING: AtomicBool = AtomicBool::new(false);
static READY: AtomicBool = AtomicBool::new(false);

fn level() -> LevelFilter {
    match option_env!("LOG").unwrap_or("info") {
        "error" => LevelFilter::Error,
        "warn" => LevelFilter::Warn,
        "info" => LevelFilter::Info,
        "debug" => LevelFilter::Debug,
        "trace" => LevelFilter::Trace,
        _ => LevelFilter::Off,
    }
}

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorCode {
    Red = 31,
    Green = 32,
    Yellow = 33,
    Cyan = 36,
    BrightBlack = 90,
    BrightRed = 91,
    BrightGreen = 92,
    BrightYellow = 93,
    BrightCyan = 96,
}

impl Display for ColorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "\u{1B}[{}m", *self as u8)
    }
}

struct Logger;
impl Log for Logger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata())
            || !READY.load(Ordering::Relaxed)
            || WRITING
                .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
                .is_err()
        {
            return;
        }
        let level = record.level();
        let (level_color, args_color) = match level {
            Level::Error => (ColorCode::BrightRed, ColorCode::Red),
            Level::Warn => (ColorCode::BrightYellow, ColorCode::Yellow),
            Level::Info => (ColorCode::BrightGreen, ColorCode::Green),
            Level::Debug => (ColorCode::BrightCyan, ColorCode::Cyan),
            Level::Trace => (ColorCode::BrightBlack, ColorCode::BrightBlack),
        };
        let reset = "\x1b[0m";
        // Preserve the original colored level and message, without allocation.
        let _ = super::console::Writer.write_fmt(format_args!(
            "[{level_color}{level}{reset} {}:{}] {args_color}{}{reset}\n",
            record.file().unwrap_or("?"),
            record.line().unwrap_or(0),
            record.args()
        ));
        WRITING.store(false, Ordering::Release);
    }
    fn flush(&self) {}
}

pub fn init() {
    let _ = log::set_logger(&Logger);
    READY.store(super::console::init(), Ordering::Relaxed);
    log::set_max_level(level());
}

#[cfg(feature = "kernel-test")]
pub fn panic_with_lock_held() -> ! {
    WRITING.store(true, Ordering::Relaxed);
    panic!("Kernel injected panic while logger locked");
}
