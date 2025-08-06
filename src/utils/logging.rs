use core::fmt::Write;
use core::fmt::{self, Display};
use log::{Level, LevelFilter, Log, Metadata, Record};


pub struct SimpleLogger;

impl Write for SimpleLogger {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for c in s.chars() {
            super::console::console_putchar(c as usize);
        }
        Ok(())
    }
}

// 实现 print! 和 println! 宏
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::logging::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", format_args!($($arg)*)));
}

pub fn _print(args: fmt::Arguments) {
    SimpleLogger.write_fmt(args).unwrap();
}

pub fn log_init() {
    static LOGGER: SimpleLogger = SimpleLogger;
    log::set_logger(&LOGGER).unwrap();
    log::set_max_level(match option_env!("LOG") {
        Some("error") => LevelFilter::Error,
        Some("warn") => LevelFilter::Warn,
        Some("info") => LevelFilter::Info,
        Some("debug") => LevelFilter::Debug,
        Some("trace") => LevelFilter::Trace,
        _ => LevelFilter::Off,
    });
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

impl Log for SimpleLogger {
    fn enabled(&self, _metadata: &Metadata) -> bool {
        true
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let level = record.level();
        let file = record.file().unwrap_or("none");
        let line = record.line().unwrap_or(0);
        let args = record.args();
        let color_reset = "\u{1B}[0m";

        // 获取对应级别的颜色
        let (level_color, args_color) = match level {
            Level::Error => (ColorCode::BrightRed, ColorCode::Red),
            Level::Warn => (ColorCode::BrightYellow, ColorCode::Yellow),
            Level::Info => (ColorCode::BrightGreen, ColorCode::Green),
            Level::Debug => (ColorCode::BrightCyan, ColorCode::Cyan),
            Level::Trace => (ColorCode::BrightBlack, ColorCode::BrightBlack),
        };

        // 彩色输出格式：[级别 文件:行号] 消息
        println!(
            "[{level_color}{level}{color_reset} {file}:{line}] {args_color}{args}{color_reset}",
        );
    }

    fn flush(&self) {}
}
