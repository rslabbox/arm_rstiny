//! Top status panel: brand text on the left, the shutdown button on the
//! right. The panel draws above the windows, and its button hit-tests
//! before the window layer ever sees the click.

use rstiny_gui::{rgb, Canvas};

/// Panel height in pixels.
pub const BAR_H: i32 = 22;

/// Shutdown button rect as (x, y, w, h), top right of the bar.
pub fn button_rect(width: i32) -> (i32, i32, i32, i32) {
    (width - 64, 3, 56, BAR_H - 6)
}

/// Draw the panel above everything except the cursor.
pub fn render(canvas: &mut Canvas) {
    let width = canvas.width() as i32;
    canvas.fill_rect(0, 0, width as usize, BAR_H as usize, rgb(0x0c, 0x16, 0x22));
    canvas.fill_rect(0, (BAR_H - 1) as usize, width as usize, 1, rgb(0x2c, 0x3e, 0x52));
    canvas.draw_text("rstiny", 8, 6, rgb(0xd8, 0xe4, 0xf0));
    let (bx, by, bw, bh) = button_rect(width);
    canvas.fill_rect(bx as usize, by as usize, bw as usize, bh as usize, rgb(0x70, 0x2a, 0x2a));
    canvas.draw_text("off", (bx + bw / 2 - 12) as usize, (by + 4) as usize, rgb(0xff, 0xd8, 0xd8));
}

/// Whether a click at (x, y) hits the shutdown button.
pub fn hits_shutdown(width: i32, x: i32, y: i32) -> bool {
    let (bx, by, bw, bh) = button_rect(width);
    x >= bx && x < bx + bw && y >= by && y < by + bh
}
