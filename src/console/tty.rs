//! Simple TTY service backed by UART with interrupt-driven input.
//!
//! This provides a minimal terminal service that reads input from a ring buffer
//! (filled by UART IRQ handler), echoes input, and dispatches commands to the
//! user command system.
//!
//! Architecture decisions:
//! - Runs as a regular task via `task::thread::spawn`, so it cooperates
//!   with the kernel scheduler.
//! - Input is interrupt-driven: UART IRQ handler pushes characters to
//!   `crate::drivers::uart::UART_INPUT` ring buffer.
//! - TTY task consumes from the buffer, providing proper echo and line editing.
//! - Commands are handled by the modular `crate::user` command system.
//! - Supports command history navigation with Up/Down arrow keys.

use alloc::string::String;

use crate::drivers::uart::{putchar, puts};
use crate::user::commands::history::HISTORY;

/// ANSI escape sequence state machine.
#[derive(Clone, Copy, PartialEq)]
enum EscapeState {
    Normal,
    Escape,  // Got ESC (0x1B)
    Bracket, // Got ESC [
}

/// Start a background tty task. Returns immediately after spawning.
pub fn start_tty() {
    // Spawn as a detached thread/task. Ignore the JoinHandle.
    let _ = crate::task::thread::spawn("tty", || {
        tty_main();
    });
}

fn tty_main() {
    puts("\r\n[tty] started. Type 'help' for commands.\r\n");
    puts("> ");

    let mut line = String::new();
    let mut esc_state = EscapeState::Normal;

    loop {
        if let Some(c) = crate::drivers::uart::getchar() {
            match esc_state {
                EscapeState::Normal => {
                    match c {
                        0x1B => {
                            // ESC character - start escape sequence
                            esc_state = EscapeState::Escape;
                        }
                        b'\r' | b'\n' => {
                            // Echo newline
                            puts("\r\n");

                            // Add to history before executing
                            if !line.trim().is_empty() {
                                HISTORY.lock().push(line.trim());
                            }

                            // Reset history navigation
                            HISTORY.lock().reset_navigation();

                            // Handle the command
                            handle_line(&line);

                            // Clear line buffer
                            line.clear();

                            // Print prompt
                            puts("> ");
                        }
                        8 | 127 => {
                            // Backspace
                            if !line.is_empty() {
                                line.pop();
                                // Move cursor back, overwrite with space, move back again
                                puts("\x08 \x08");
                            }
                        }
                        c if c.is_ascii_graphic() || c == b' ' => {
                            // Printable character - reset history navigation
                            HISTORY.lock().reset_navigation();
                            line.push(c as char);
                            putchar(c);
                        }
                        _ => {
                            // Ignore other control characters
                        }
                    }
                }
                EscapeState::Escape => {
                    if c == b'[' {
                        esc_state = EscapeState::Bracket;
                    } else {
                        // Invalid escape sequence, reset
                        esc_state = EscapeState::Normal;
                    }
                }
                EscapeState::Bracket => {
                    esc_state = EscapeState::Normal;
                    match c {
                        b'A' => {
                            // Up arrow - previous history
                            handle_history_prev(&mut line);
                        }
                        b'B' => {
                            // Down arrow - next history
                            handle_history_next(&mut line);
                        }
                        b'C' => {
                            // Right arrow - ignore for now
                        }
                        b'D' => {
                            // Left arrow - ignore for now
                        }
                        _ => {
                            // Unknown escape sequence, ignore
                        }
                    }
                }
            }
        } else {
            // No input available, yield to other tasks
            crate::task::thread::yield_now();
        }
    }
}

/// Clear current line on terminal and redraw with new content.
fn redraw_line(old_len: usize, new_line: &str) {
    // Move cursor to start of input (after prompt)
    for _ in 0..old_len {
        puts("\x08"); // Move back
    }
    // Clear old content
    for _ in 0..old_len {
        puts(" ");
    }
    // Move back again
    for _ in 0..old_len {
        puts("\x08");
    }
    // Print new content
    puts(new_line);
}

/// Handle up arrow - navigate to previous history entry.
fn handle_history_prev(line: &mut String) {
    let old_len = line.len();
    let mut history = HISTORY.lock();

    if let Some(prev) = history.prev(line) {
        let prev_owned = String::from(prev);
        drop(history); // Release lock before redrawing

        redraw_line(old_len, &prev_owned);
        line.clear();
        line.push_str(&prev_owned);
    }
}

/// Handle down arrow - navigate to next history entry.
fn handle_history_next(line: &mut String) {
    let old_len = line.len();
    let mut history = HISTORY.lock();

    if let Some(next) = history.next() {
        let next_owned = String::from(next);
        drop(history); // Release lock before redrawing

        redraw_line(old_len, &next_owned);
        line.clear();
        line.push_str(&next_owned);
    }
}

fn handle_line(cmd: &str) {
    if cmd.is_empty() {
        return;
    }

    // Dispatch to the user command system
    crate::user::execute(cmd);
}
