#![no_std]
//! Minimal software rasterizer over a leased framebuffer
//! (docs/gui-display.md §3): solid rectangles, bitmap text and line scroll.
//! Everything writes the caller's pixel words in place — 32 bits per pixel,
//! little-endian `0xAARRGGBB`, row stride `width`. `tools/check_gpu.py`
//! reimplements these exact rules host-side, so the drawing is verifiable.

pub mod font;

/// Pack an RGB triple into the opaque wire format (`0xAARRGGBB`).
pub const fn rgb(red: u8, green: u8, blue: u8) -> u32 {
    0xFF00_0000 | (red as u32) << 16 | (green as u32) << 8 | blue as u32
}

/// A full-screen 32-bpp canvas over a leased framebuffer.
pub struct Canvas<'a> {
    pixels: &'a mut [u32],
    width: usize,
    height: usize,
}

impl<'a> Canvas<'a> {
    /// Wrap the pixel words of a framebuffer of `width * height` pixels.
    ///
    /// # Safety
    /// `buffer` must address `width * height * 4` mapped, writable bytes that
    /// nothing else aliases while the canvas lives.
    pub unsafe fn new(buffer: *mut u8, width: usize, height: usize) -> Self {
        Canvas {
            // SAFETY: guaranteed by the caller, per the contract above.
            pixels: unsafe {
                core::slice::from_raw_parts_mut(
                    buffer as *mut u32,
                    width.checked_mul(height).expect("sane resolution"),
                )
            },
            width,
            height,
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Solid rectangle, clipped to the canvas.
    pub fn fill_rect(&mut self, x: usize, y: usize, w: usize, h: usize, color: u32) {
        for row in y..(y + h).min(self.height) {
            let Some(row) = row.checked_mul(self.width) else {
                break;
            };
            for column in x..(x + w).min(self.width) {
                let Some(index) = row.checked_add(column) else {
                    break;
                };
                if let Some(pixel) = self.pixels.get_mut(index) {
                    *pixel = color;
                }
            }
        }
    }

    /// One bitmap text line, top-left at `(x, y)`. Characters outside the
    /// table render as `?`; the line clips at the right/bottom edge.
    pub fn draw_text(&mut self, text: &str, x: usize, y: usize, color: u32) {
        let mut pen = x;
        for character in text.bytes() {
            let glyph = glyph_of(character);
            for (row, bits) in glyph.iter().enumerate() {
                for column in 0..font::GLYPH_W {
                    if bits & (1 << column) == 0 {
                        continue;
                    }
                    let px = pen + column;
                    let py = y + row;
                    if px >= self.width || py >= self.height {
                        continue;
                    }
                    self.pixels[py * self.width + px] = color;
                }
            }
            pen += font::GLYPH_W;
        }
    }

    /// Move every pixel up `rows` lines and clear the revealed strip with
    /// `background` (terminal-style scroll).
    pub fn scroll_up(&mut self, rows: usize, background: u32) {
        let rows = rows.min(self.height);
        let keep = self.height - rows;
        self.pixels.copy_within(rows * self.width.., 0);
        let tail = keep * self.width..self.width * self.height;
        self.pixels[tail].fill(background);
    }
}

/// The table entry for `character`, or `?` for anything outside the table.
pub fn glyph_of(character: u8) -> [u8; font::GLYPH_H] {
    let index = character.wrapping_sub(font::FIRST) as usize;
    if character < font::FIRST || index >= font::GLYPHS.len() {
        return font::GLYPHS[b'?' as usize - font::FIRST as usize];
    }
    font::GLYPHS[index]
}

/// Wrapping u32 sum over the little-endian pixel words: the guest-side
/// checksum the GPU acceptances compare against the host model.
pub fn word_sum(bytes: &[u8]) -> u32 {
    bytes.chunks_exact(4).fold(0u32, |sum, word| {
        sum.wrapping_add(u32::from_le_bytes(word.try_into().expect("4 bytes")))
    })
}

/// Title bar height and border thickness for [`draw_window`].
pub const TITLE_H: usize = 18;
pub const BORDER: usize = 2;

/// A desktop window: 2 px frame, a title bar with the caption, a client
/// area. The focused window draws a bright frame and title bar.
pub fn draw_window(
    canvas: &mut Canvas,
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    title: &str,
    focused: bool,
    client: u32,
) {
    let frame = if focused {
        rgb(0xF0, 0xF0, 0xF0)
    } else {
        rgb(0x70, 0x70, 0x78)
    };
    let bar = if focused {
        rgb(0x28, 0x50, 0x90)
    } else {
        rgb(0x40, 0x44, 0x50)
    };
    canvas.fill_rect(x, y, w, h, frame);
    canvas.fill_rect(
        x + BORDER,
        y + BORDER,
        w - 2 * BORDER,
        h - 2 * BORDER,
        frame,
    );
    canvas.fill_rect(
        x + BORDER,
        y + TITLE_H,
        w - 2 * BORDER,
        h - TITLE_H - BORDER,
        client,
    );
    canvas.fill_rect(
        x + BORDER,
        y + BORDER,
        w - 2 * BORDER,
        TITLE_H - BORDER,
        bar,
    );
    canvas.draw_text(title, x + 8, y + BORDER + 3, rgb(0xFF, 0xFF, 0xFF));
}

/// A labelled push button.
pub fn draw_button(canvas: &mut Canvas, x: usize, y: usize, w: usize, h: usize, label: &str) {
    canvas.fill_rect(x, y, w, h, rgb(0xD0, 0xD4, 0xDC));
    let tw = label.len() * font::GLYPH_W;
    let tx = x + (w.saturating_sub(tw)) / 2;
    let ty = y + (h.saturating_sub(font::GLYPH_H)) / 2;
    canvas.draw_text(label, tx, ty, rgb(0x10, 0x10, 0x14));
}

/// Software mouse cursor: an 8x12 arrow, drawn last so it stays on top.
pub fn draw_cursor(canvas: &mut Canvas, x: usize, y: usize) {
    const ARROW: [u8; 12] = [
        0b10000000, 0b11000000, 0b11100000, 0b11110000, 0b11111000, 0b11111100, 0b11111110,
        0b11111000, 0b11111100, 0b11011100, 0b10001110, 0b00000110,
    ];
    let white = rgb(0xFF, 0xFF, 0xFF);
    let black = rgb(0x00, 0x00, 0x00);
    for (row, bits) in ARROW.iter().enumerate() {
        for column in 0..8 {
            let px = x + column;
            let py = y + row;
            if px >= canvas.width || py >= canvas.height {
                continue;
            }
            let edge = bits & (1 << (7 - column)) != 0
                && (bits << 1 & (1 << (7 - column)) == 0 || column == 7);
            let color = if bits & (1 << (7 - column)) != 0 {
                if edge { black } else { white }
            } else {
                continue;
            };
            canvas.pixels[py * canvas.width + px] = color;
        }
    }
}
