//! The `wm` scene: a tiny two-window desktop — a keypad calculator and a
//! txt editor — on the leased framebuffer (docs/gui-display.md §10). The
//! client composites both windows plus a software cursor and handles the
//! input itself: the mouse drags windows by their title bars, the keyboard
//! types into the focused one, and the arrow keys move it. There is no
//! compositor in the server; everything below runs in the client's address
//! space, per the single-client full-screen lease design.

use crate::key_char;
use rstiny::ipc;
use rstiny_gui::{Canvas, draw_button, draw_cursor, draw_window, rgb};
use rstiny_protocol::{gpu, status};
use rstiny_server::{Service, logln};

/// Window indices and identities.
pub const CALC: usize = 0;
pub const EDIT: usize = 1;
const NAMES: [&str; 2] = ["calculator", "editor"];
const TITLES: [&str; 2] = ["calculator", "editor - txt"];

const CALC_W: usize = 208;
const CALC_H: usize = 240;
const EDIT_W: usize = 320;
const EDIT_H: usize = 208;

/// Input event types (evdev): key press/release and relative motion.
const EV_KEY: u64 = 1;
const EV_REL: u64 = 2;
const REL_X: u64 = 0;
const BTN_LEFT: u64 = 272; // 0x110
const MOVE_STEP: i32 = 8;
const POLL_MS: u64 = 20;

/// One desktop window's geometry.
#[derive(Clone, Copy)]
struct Win {
    x: i32,
    y: i32,
    w: usize,
    h: usize,
}

impl Win {
    fn contains(&self, cx: i32, cy: i32) -> bool {
        cx >= self.x && cx < self.x + self.w as i32 && cy >= self.y && cy < self.y + self.h as i32
    }
    fn title_contains(&self, cx: i32, cy: i32) -> bool {
        cx >= self.x
            && cx < self.x + self.w as i32
            && cy >= self.y
            && cy < self.y + rstiny_gui::TITLE_H as i32
    }
}


/// Calculator state: left-to-right evaluation with `=`.
#[derive(Default)]
struct Calc {
    acc: u32,
    entry: u32,
    op: Option<u8>,
}

impl Calc {
    fn digit(&mut self, d: u32) {
        self.entry = self.entry.saturating_mul(10).saturating_add(d);
    }
    fn apply(&mut self, op: u8) {
        let rhs = self.entry;
        self.acc = match self.op.unwrap_or(op) {
            b'-' => self.acc.saturating_sub(rhs),
            b'*' => self.acc.wrapping_mul(rhs),
            b'/' => {
                if rhs > 0 {
                    self.acc / rhs
                } else {
                    self.acc
                }
            }
            _ => self.acc.saturating_add(rhs),
        };
        self.entry = 0;
    }
    fn push(&mut self, key: u8) -> bool {
        match key {
            b'0'..=b'9' => {
                self.digit(u32::from(key - b'0'));
                true
            }
            b'+' | b'-' | b'*' | b'/' => {
                self.apply(key);
                self.op = Some(key);
                true
            }
            b'=' => {
                if let Some(op) = self.op.take() {
                    self.apply(op);
                }
                true
            }
            _ => false,
        }
    }
    fn display(&self) -> u32 {
        if self.entry > 0 { self.entry } else { self.acc }
    }
}

/// What the caller should do after an input event.
#[derive(PartialEq, Eq)]
pub enum Effect {
    None,
    Redraw,
}

/// The whole desktop state.
pub struct Wm {
    width: i32,
    wins: [Win; 2],
    focus: usize,
    cursor: (i32, i32),
    drag: Option<(usize, i32, i32)>,
    calc: Calc,
    edit: heapless::Buffer,
    exit: bool,
}

impl Wm {
    pub fn new(width: i32, height: i32) -> Self {
        Wm {
            width,
            wins: [
                Win {
                    x: 40,
                    y: 60,
                    w: CALC_W,
                    h: CALC_H,
                },
                Win {
                    x: 300,
                    y: 120,
                    w: EDIT_W,
                    h: EDIT_H,
                },
            ],
            focus: CALC,
            cursor: (width / 2, height / 2),
            drag: None,
            calc: Calc::default(),
            edit: heapless::Buffer::new(),
            exit: false,
        }
    }


    /// Process one packed INPUT_READ event. Returns the effect on the
    /// framebuffer and logs state changes through the console.
    pub fn handle(&mut self, word: u64, service: &Service) -> Effect {
        if word >> 63 == 0 {
            return Effect::None;
        }
        let kind = (word >> 40) & 0xFFFF;
        let code = (word >> 24) & 0xFFFF;
        let raw = (word & 0xFF_FFFF) as i32;
        let value = (raw << 8) >> 8; // sign-extend 24 bits

        if kind == EV_REL {
            let (dx, dy) = if code == REL_X {
                (value, 0)
            } else {
                (0, value)
            };
            self.cursor.0 += dx;
            self.cursor.1 += dy;
            // keep the cursor within the screen
            if let Some((win, ..)) = self.drag {
                self.wins[win].x += dx;
                self.wins[win].y += dy;
                logln!(
                    service,
                    "[gui] {} move {} {}",
                    NAMES[win],
                    self.wins[win].x,
                    self.wins[win].y
                );
            }
            return Effect::Redraw;
        }
        if kind != EV_KEY {
            return Effect::None;
        }
        if code == BTN_LEFT {
            if value == 1 {
                // The panel owns the top strip: the shutdown button powers
                // the machine off, and bar clicks never reach the windows.
                if crate::panel::hits_shutdown(self.width, self.cursor.0, self.cursor.1) {
                    logln!(service, "[gui] shutdown requested");
                    rstiny::poweroff();
                }
                if self.cursor.1 < crate::panel::BAR_H {
                    return Effect::Redraw;
                }
                // Press: focus whatever is under the cursor, drag by the
                // title bar.
                for (order, index) in [self.focus, 1 - self.focus].into_iter().enumerate() {
                    if order == 1 && self.wins[index].contains(self.cursor.0, self.cursor.1) {
                        self.focus = index;
                    }
                }
                for index in 0..2 {
                    if self.wins[index].title_contains(self.cursor.0, self.cursor.1) {
                        self.focus = index;
                        self.drag = Some((
                            index,
                            self.cursor.0 - self.wins[index].x,
                            self.cursor.1 - self.wins[index].y,
                        ));
                        logln!(service, "[gui] focus {}", NAMES[index]);
                        break;
                    }
                }
            } else {
                self.drag = None;
            }
            return Effect::Redraw;
        }
        // Keyboard. Only the press edge acts: without this filter every
        // keypress arrives twice (press + release), doubling characters in
        // the editor and duplicating calculator entries.
        if value == 0 {
            return Effect::None;
        }
        match code {
            103 => return self.move_focused(0, -MOVE_STEP, service),
            105 => return self.move_focused(-MOVE_STEP, 0, service),
            106 => return self.move_focused(MOVE_STEP, 0, service),
            108 => return self.move_focused(0, MOVE_STEP, service),
            15 => {
                self.focus = 1 - self.focus;
                logln!(service, "[gui] focus {}", NAMES[self.focus]);
                return Effect::Redraw;
            }
            1 => {
                self.exit = true;
                return Effect::None;
            }
            14 => {
                if self.focus == EDIT && self.edit.pop() {
                    logln!(service, "[gui] edit {}", self.edit.as_str());
                    return Effect::Redraw;
                }
                return Effect::None;
            }
            _ => {}
        }
        if self.focus == CALC {
            if let Some(key) = keypad(code).or_else(|| digit_char(code)) {
                if self.calc.push(key) {
                    logln!(service, "[gui] calc {}", self.calc.display());
                    return Effect::Redraw;
                }
            }
        } else if let Some(character) = key_char(code as u8).filter(|c| *c != '\n') {
            if code == 57 || character.is_ascii_graphic() {
                self.edit.push(character as u8);
                logln!(service, "[gui] edit {}", self.edit.as_str());
                return Effect::Redraw;
            }
        }
        Effect::None
    }

    fn move_focused(&mut self, dx: i32, dy: i32, service: &Service) -> Effect {
        self.wins[self.focus].x += dx;
        self.wins[self.focus].y += dy;
        logln!(
            service,
            "[gui] {} move {} {}",
            NAMES[self.focus],
            self.wins[self.focus].x,
            self.wins[self.focus].y
        );
        Effect::Redraw
    }

    pub fn exit_requested(&self) -> bool {
        self.exit
    }

    /// Composite the desktop: background, both windows (focused last), the
    /// editor text and the cursor.
    pub fn render(&self, canvas: &mut Canvas) {
        canvas.fill_rect(0, 0, canvas.width(), canvas.height(), rgb(0x18, 0x22, 0x30));
        for index in 0..2 {
            let win = &self.wins[index];
            let focused = index == self.focus;
            if index == CALC {
                draw_window(
                    canvas,
                    win.x as usize,
                    win.y as usize,
                    win.w,
                    win.h,
                    TITLES[CALC],
                    focused,
                    rgb(0xE8, 0xEA, 0xEE),
                );
                self.render_calc(canvas, win.x as usize, win.y as usize, focused);
            } else {
                draw_window(
                    canvas,
                    win.x as usize,
                    win.y as usize,
                    win.w,
                    win.h,
                    TITLES[EDIT],
                    focused,
                    rgb(0x10, 0x12, 0x18),
                );
                self.render_edit(canvas, win.x as usize, win.y as usize);
            }
        }
        crate::panel::render(canvas);
        draw_cursor(
            canvas,
            self.cursor.0.max(0) as usize,
            self.cursor.1.max(0) as usize,
        );
    }

    fn render_calc(&self, canvas: &mut Canvas, x: usize, y: usize, focused: bool) {
        let text_color = rgb(0x00, 0xFF, 0x60);
        let mut display = [0u8; 12];
        let text = u32_to_str(self.calc.display(), &mut display);
        canvas.fill_rect(x + 10, y + 26, CALC_W - 20, 24, rgb(0x00, 0x20, 0x10));
        canvas.draw_text(text, x + 14, y + 30, text_color);
        let labels = [
            ["7", "8", "9", "/"],
            ["4", "5", "6", "*"],
            ["1", "2", "3", "-"],
            ["0", ".", "=", "+"],
        ];
        for (row, row_labels) in labels.iter().enumerate() {
            for (column, label) in row_labels.iter().enumerate() {
                let bx = x + 10 + column * 46;
                let by = y + 58 + row * 42;
                draw_button(canvas, bx, by, 42, 36, label);
            }
        }
        let _ = focused;
    }

    fn render_edit(&self, canvas: &mut Canvas, x: usize, y: usize) {
        let color = rgb(0xC0, 0xE0, 0xFF);
        let mut row = 0;
        let mut start = 0;
        for index in 0..=self.edit.len() {
            if index == self.edit.len() || self.edit.byte_at(index) == b'\n' {
                canvas.draw_text(
                    self.edit.slice(start, index),
                    x + 10,
                    y + 26 + row * 16,
                    color,
                );
                start = index + 1;
                row += 1;
            }
        }
    }
}

/// Fixed-capacity single-heap-free text buffer for the editor.
mod heapless {
    pub struct Buffer {
        bytes: [u8; 192],
        len: usize,
    }
    impl Buffer {
        pub fn new() -> Self {
            Buffer {
                bytes: [0; 192],
                len: 0,
            }
        }
        pub fn push(&mut self, byte: u8) -> bool {
            if self.len < self.bytes.len() {
                self.bytes[self.len] = byte;
                self.len += 1;
                true
            } else {
                false
            }
        }
        pub fn pop(&mut self) -> bool {
            if self.len > 0 {
                self.len -= 1;
                true
            } else {
                false
            }
        }
        pub fn len(&self) -> usize {
            self.len
        }
        pub fn byte_at(&self, index: usize) -> u8 {
            self.bytes[index]
        }
        /// The buffer as a string slice up to `end`.
        pub fn slice(&self, start: usize, end: usize) -> &str {
            match core::str::from_utf8(&self.bytes[start..end]) {
                Ok(text) => text,
                Err(_) => "",
            }
        }
        pub fn as_str(&self) -> &str {
            self.slice(0, self.len)
        }
    }
}

/// Map keypad scancodes to calculator keys.
fn keypad(code: u64) -> Option<u8> {
    match code {
        71 => Some(b'7'),
        72 => Some(b'8'),
        73 => Some(b'9'),
        74 => Some(b'-'),
        75 => Some(b'4'),
        76 => Some(b'5'),
        77 => Some(b'6'),
        78 => Some(b'+'),
        79 => Some(b'1'),
        80 => Some(b'2'),
        81 => Some(b'3'),
        82 => Some(b'0'),
        83 => Some(b'.'),
        96 => Some(b'='),
        _ => None,
    }
}

/// Main-row digits (for the calculator / typing).
fn digit_char(code: u64) -> Option<u8> {
    match code {
        2..=10 => Some(b'1' + (code - 2) as u8),
        11 => Some(b'0'),
        _ => None,
    }
}

fn u32_to_str(mut value: u32, out: &mut [u8]) -> &str {
    if value == 0 {
        out[0] = b'0';
        return core::str::from_utf8(&out[..1]).unwrap_or("0");
    }
    let mut digits = [0u8; 10];
    let mut count = 0;
    while value > 0 {
        digits[count] = b'0' + (value % 10) as u8;
        value /= 10;
        count += 1;
    }
    for (index, digit) in digits[..count].iter().rev().enumerate() {
        out[index] = *digit;
    }
    core::str::from_utf8(&out[..count]).unwrap_or("")
}

/// Run the wm scene: composite, poll INPUT_READ, route events, flush. Logs
/// state transitions through the console; runs until Esc.
pub fn run(service: &Service, canvas: &mut Canvas, gpu_ep: u64, width: usize, height: usize) {
    let mut wm = Wm::new(width as i32, height as i32);
    wm.render(canvas);
    let _ = ipc::call(gpu_ep, gpu::FLUSH, &[0, 0, width as u64, height as u64]);
    logln!(service, "[gui] wm ready: calculator + editor");
    // The desktop runs until Esc: termination relies on the exit request,
    // not a poll budget (the bound forced the desktop to quit mid-session).
    loop {
        if wm.exit_requested() {
            break;
        }
        let _ = rstiny::sleep(POLL_MS);
        let mut dirty = false;
        for _ in 0..32 {
            let reply = ipc::call(gpu_ep, gpu::INPUT_READ, &[])
                .ok()
                .filter(|reply| reply.label == status::OK);
            let Some(reply) = reply else { break };
            if reply.word(0) == 0 {
                break;
            }
            match wm.handle(reply.word(0), service) {
                Effect::Redraw => dirty = true,
                Effect::None => {}
            }
        }
        if dirty {
            wm.render(canvas);
            let _ = ipc::call(gpu_ep, gpu::FLUSH, &[0, 0, width as u64, height as u64]);
        }
    }
    logln!(service, "[gui] wm exit");
}
