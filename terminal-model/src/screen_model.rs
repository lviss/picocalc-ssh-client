use embedded_graphics::mono_font::MonoFont;
use embedded_graphics::pixelcolor::{Rgb565, Rgb888};
use embedded_graphics::prelude::*;
use vte::Perform;

use alloc::vec;
use alloc::vec::Vec;

pub const SCREEN_HEIGHT: u16 = 320;
pub const SCREEN_WIDTH: u16 = 320;

static FONTS: &[&MonoFont] = &[
    &profont::PROFONT_7_POINT,
    &profont::PROFONT_9_POINT,
    &profont::PROFONT_10_POINT,
    &profont::PROFONT_12_POINT,
    &profont::PROFONT_14_POINT,
    &profont::PROFONT_18_POINT,
    &profont::PROFONT_24_POINT,
];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
    DefaultFg,
    DefaultBg,
    Rgb(u8, u8, u8),
    Indexed(u8),
}

impl Color {
    pub fn to_rgb565(self, is_bg: bool) -> Rgb565 {
        match self {
            Color::Black => Rgb565::BLACK,
            Color::Red => Rgb565::RED,
            Color::Green => Rgb565::GREEN,
            Color::Yellow => Rgb565::YELLOW,
            Color::Blue => Rgb565::BLUE,
            Color::Magenta => Rgb565::MAGENTA,
            Color::Cyan => Rgb565::CYAN,
            Color::White => Rgb565::WHITE,
            Color::BrightBlack => Rgb565::new(10, 20, 10), // Approx
            Color::BrightRed => Rgb565::new(31, 20, 20),
            Color::BrightGreen => Rgb565::new(20, 63, 20),
            Color::BrightYellow => Rgb565::new(31, 63, 20),
            Color::BrightBlue => Rgb565::new(20, 20, 31),
            Color::BrightMagenta => Rgb565::new(31, 20, 31),
            Color::BrightCyan => Rgb565::new(20, 63, 31),
            Color::BrightWhite => Rgb565::WHITE,
            Color::DefaultFg => Rgb565::CSS_LIGHT_GRAY,
            Color::DefaultBg => Rgb565::BLACK,
            Color::Rgb(r, g, b) => Rgb888::new(r, g, b).into(),
            Color::Indexed(i) => {
                // Simple mapping for first 16 colors, else default
                if i < 8 {
                    // map to standard colors
                    match i {
                        0 => Rgb565::BLACK,
                        1 => Rgb565::RED,
                        2 => Rgb565::GREEN,
                        3 => Rgb565::YELLOW,
                        4 => Rgb565::BLUE,
                        5 => Rgb565::MAGENTA,
                        6 => Rgb565::CYAN,
                        7 => Rgb565::CSS_LIGHT_GRAY,
                        _ => Rgb565::WHITE,
                    }
                } else if i < 16 {
                    // brights
                     match i {
                        8 => Rgb565::new(10, 20, 10),
                        9 => Rgb565::new(31, 20, 20),
                        10 => Rgb565::new(20, 63, 20),
                        11 => Rgb565::new(31, 63, 20),
                        12 => Rgb565::new(20, 20, 31),
                        13 => Rgb565::new(31, 20, 31),
                        14 => Rgb565::new(20, 63, 31),
                        15 => Rgb565::WHITE,
                        _ => Rgb565::WHITE,
                    }
                } else {
                    if is_bg { Rgb565::BLACK } else { Rgb565::WHITE }
                }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Attrs {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub underline: bool,
    pub reverse: bool,
}

impl Default for Attrs {
    fn default() -> Self {
        Self {
            fg: Color::DefaultFg,
            bg: Color::DefaultBg,
            bold: false,
            underline: false,
            reverse: false,
        }
    }
}

#[derive(Clone)]
pub struct ScreenLine {
    pub chars: Vec<char>,
    pub attrs: Vec<Attrs>,
    pub dirty: bool,
}

impl ScreenLine {
    fn new(width: usize) -> Self {
        Self {
            chars: vec![' '; width],
            attrs: vec![Attrs::default(); width],
            dirty: true,
        }
    }

    fn clear(&mut self) {
        for c in self.chars.iter_mut() { *c = ' '; }
        for a in self.attrs.iter_mut() { *a = Attrs::default(); }
        self.dirty = true;
    }
}

pub struct ScreenModel {
    pub lines: Vec<ScreenLine>,
    pub scrollback: Vec<ScreenLine>,
    pub viewport_offset: usize,
    max_scrollback: usize,
    pub cursor_x: usize,
    pub cursor_y: usize,
    current_attrs: Attrs,
    pub font: &'static MonoFont<'static>,
    pub rows: usize,
    cols: usize,
    pub full_repaint: bool,
}

impl Default for ScreenModel {
    fn default() -> Self {
        let font = FONTS[2];
        let cols = ((SCREEN_WIDTH as u32) / (font.character_size.width + font.character_spacing)) as usize;
        let rows = ((SCREEN_HEIGHT as u32) / font.character_size.height) as usize;

        // Initialize lines
        let mut lines = Vec::with_capacity(rows);
        for _ in 0..rows {
            lines.push(ScreenLine::new(cols));
        }

        Self {
            lines,
            scrollback: Vec::new(),
            viewport_offset: 0,
            max_scrollback: 200,
            cursor_x: 0,
            cursor_y: 0,
            current_attrs: Attrs::default(),
            font,
            rows,
            cols,
            full_repaint: true,
        }
    }
}

impl ScreenModel {
    pub fn width(&self) -> u16 {
        self.cols as u16
    }

    pub fn height(&self) -> u16 {
        self.rows as u16
    }

    pub fn clear(&mut self) {
        for line in self.lines.iter_mut() {
            line.clear();
        }
        self.cursor_x = 0;
        self.cursor_y = 0;
        self.full_repaint = true;
    }

    pub fn increase_font(&mut self) {
        // TODO: implement font resizing
    }

    pub fn decrease_font(&mut self) {
        // TODO: implement font resizing
    }

    fn scroll_up(&mut self) {
        // Remove first line, add new line at end
        if !self.lines.is_empty() {
            let line = self.lines.remove(0);
            self.scrollback.push(line);
            if self.scrollback.len() > self.max_scrollback {
                self.scrollback.remove(0);
            }
            self.lines.push(ScreenLine::new(self.cols));
            self.full_repaint = true;
        }
    }

    pub fn scroll_view_up(&mut self, n: usize) {
        self.viewport_offset = (self.viewport_offset + n).min(self.scrollback.len());
        self.full_repaint = true;
    }

    pub fn scroll_view_down(&mut self, n: usize) {
        self.viewport_offset = self.viewport_offset.saturating_sub(n);
        self.full_repaint = true;
    }

    pub fn reset_view(&mut self) {
        if self.viewport_offset != 0 {
            self.viewport_offset = 0;
            self.full_repaint = true;
        }
    }

    pub fn set_max_scrollback(&mut self, max: usize) {
        self.max_scrollback = max;
        if self.scrollback.len() > max {
            let remove_count = self.scrollback.len() - max;
            self.scrollback.drain(0..remove_count);
            // Adjust viewport offset if it's now out of bounds
            if self.viewport_offset > self.scrollback.len() {
                self.viewport_offset = self.scrollback.len();
            }
        }
    }
}

// vte always pushes a parameter slot on CSI dispatch, even when the sequence had no
// digits (e.g. bare `CSI C`) - the missing parameter comes through as an explicit 0,
// not an absent one. ECMA-48 treats both "absent" and "0" as "use the default value",
// so `.unwrap_or(1)` alone never fires for the common bare-sequence case; this maps
// both cases to the default of 1.
fn cursor_move_count(params: &vte::Params) -> usize {
    params.iter().next().map(|p| p[0]).unwrap_or(0).max(1) as usize
}

impl Perform for ScreenModel {
    fn print(&mut self, c: char) {
        self.reset_view();
        if self.cursor_y >= self.rows {
            self.scroll_up();
            self.cursor_y = self.rows - 1;
        }
        if self.cursor_x >= self.cols {
            self.cursor_x = 0;
            self.cursor_y += 1;
            if self.cursor_y >= self.rows {
                self.scroll_up();
                self.cursor_y = self.rows - 1;
            }
        }

        let line = &mut self.lines[self.cursor_y];
        if self.cursor_x < line.chars.len() {
            line.chars[self.cursor_x] = c;
            line.attrs[self.cursor_x] = self.current_attrs;
            line.dirty = true;
            self.cursor_x += 1;
        }
    }

    fn execute(&mut self, byte: u8) {
        self.reset_view();
        match byte {
            b'\n' => { // LF
                self.cursor_y += 1;
                if self.cursor_y >= self.rows {
                    self.scroll_up();
                    self.cursor_y = self.rows - 1;
                }
            }
            b'\r' => { // CR
                self.cursor_x = 0;
            }
            b'\x08' => { // BS
                if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                }
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &vte::Params, intermediates: &[u8], ignore: bool, action: char) {
        if ignore || !intermediates.is_empty() { return; }

        match action {
            'A' => { // Cursor Up
                let n = cursor_move_count(params);
                self.cursor_y = self.cursor_y.saturating_sub(n);
            }
            'B' => { // Cursor Down
                let n = cursor_move_count(params);
                self.cursor_y = (self.cursor_y + n).min(self.rows - 1);
            }
            'C' => { // Cursor Forward
                // claude-code's Ink renderer represents runs of typed spaces as bare
                // CSI C instead of literal 0x20 bytes, betting that the skipped cells
                // are already blank. Strict ECMA-48 CUF must not touch cell content,
                // but that leaves stale glyphs on screen and the line non-dirty when
                // that bet is wrong, so we deliberately diverge: blank every column
                // advanced over, matching what printing that many spaces would do.
                let n = cursor_move_count(params);
                let start = self.cursor_x;
                let blank_end = (start + n).min(self.cols);
                if blank_end > start {
                    let attrs = self.current_attrs;
                    let line = &mut self.lines[self.cursor_y];
                    for i in start..blank_end {
                        line.chars[i] = ' ';
                        line.attrs[i] = attrs;
                    }
                    line.dirty = true;
                }
                self.cursor_x = (start + n).min(self.cols - 1);
            }
            'D' => { // Cursor Backward
                let n = cursor_move_count(params);
                self.cursor_x = self.cursor_x.saturating_sub(n);
            }
            'H' | 'f' => { // Cursor Position
                let mut iter = params.iter();
                let row = iter.next().map(|p| p[0]).unwrap_or(1).max(1) as usize - 1;
                let col = iter.next().map(|p| p[0]).unwrap_or(1).max(1) as usize - 1;
                self.cursor_y = row.min(self.rows - 1);
                self.cursor_x = col.min(self.cols - 1);
            }
            'J' => { // Erase in Display
                let n = params.iter().next().map(|p| p[0]).unwrap_or(0);
                match n {
                    0 => { // Cursor to end
                        // Clear current line from cursor
                        let line = &mut self.lines[self.cursor_y];
                        for i in self.cursor_x..self.cols {
                            line.chars[i] = ' ';
                            line.attrs[i] = self.current_attrs;
                        }
                        line.dirty = true;
                        // Clear lines below
                        for i in (self.cursor_y + 1)..self.rows {
                            self.lines[i].clear();
                        }
                    }
                    1 => { // Beginning to cursor
                        // Clear lines above
                        for i in 0..self.cursor_y {
                            self.lines[i].clear();
                        }
                        // Clear current line up to cursor
                        let line = &mut self.lines[self.cursor_y];
                        for i in 0..=self.cursor_x {
                            line.chars[i] = ' ';
                            line.attrs[i] = self.current_attrs;
                        }
                        line.dirty = true;
                    }
                    2 => { // Entire screen
                        self.clear();
                    }
                    _ => {}
                }
            }
            'K' => { // Erase in Line
                let n = params.iter().next().map(|p| p[0]).unwrap_or(0);
                let line = &mut self.lines[self.cursor_y];
                match n {
                    0 => { // Cursor to end
                        for i in self.cursor_x..self.cols {
                            line.chars[i] = ' ';
                            line.attrs[i] = self.current_attrs;
                        }
                    }
                    1 => { // Beginning to cursor
                        for i in 0..=self.cursor_x {
                            line.chars[i] = ' ';
                            line.attrs[i] = self.current_attrs;
                        }
                    }
                    2 => { // Entire line
                        for i in 0..self.cols {
                            line.chars[i] = ' ';
                            line.attrs[i] = self.current_attrs;
                        }
                    }
                    _ => {}
                }
                line.dirty = true;
            }
            'm' => { // SGR
                for param in params.iter() {
                    let p = param[0];
                    match p {
                        0 => self.current_attrs = Attrs::default(),
                        1 => self.current_attrs.bold = true,
                        4 => self.current_attrs.underline = true,
                        7 => self.current_attrs.reverse = true,
                        22 => self.current_attrs.bold = false,
                        24 => self.current_attrs.underline = false,
                        27 => self.current_attrs.reverse = false,
                        30..=37 => self.current_attrs.fg = Color::Indexed((p - 30) as u8),
                        39 => self.current_attrs.fg = Color::DefaultFg,
                        40..=47 => self.current_attrs.bg = Color::Indexed((p - 40) as u8),
                        49 => self.current_attrs.bg = Color::DefaultBg,
                        90..=97 => self.current_attrs.fg = Color::Indexed((p - 90 + 8) as u8),
                        100..=107 => self.current_attrs.bg = Color::Indexed((p - 100 + 8) as u8),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(model: &mut ScreenModel, bytes: &[u8]) {
        let mut parser = vte::Parser::new();
        for byte in bytes {
            parser.advance(model, *byte);
        }
    }

    // Reproduces the exact mechanism from the diagnosis report: claude-code's Ink
    // renderer sometimes sends bare `CSI n C` instead of literal spaces, betting the
    // skipped cells are already blank. This feeds a `CSI 3 C` directly to the VTE
    // parser over cells that hold real (non-blank) prior content, bypassing the need
    // for a real claude-code session, and asserts the skipped cells become blank and
    // the line is marked dirty so `update_display` will actually repaint them.
    #[test]
    fn cursor_forward_blanks_skipped_non_blank_cells() {
        let mut model = ScreenModel::default();
        feed(&mut model, b"XYZ\r");
        model.lines[0].dirty = false;

        feed(&mut model, b"\x1b[3C");

        assert_eq!(&model.lines[0].chars[0..3], &[' ', ' ', ' ']);
        assert!(model.lines[0].dirty, "line must be marked dirty so update_display repaints it");
        assert_eq!(model.cursor_x, 3);
    }

    // The report's live captures show claude-code emitting *bare* `ESC[C` (no digit)
    // for single-space skips, e.g. "ESC[21;6H ESC[C ESC[C y". vte always pushes a
    // parameter (0, not absent) even for a bare CSI, so a naive `.unwrap_or(1)` on
    // the parsed value silently computes n=0 and this handler would never fire for
    // exactly the sequence claude-code actually sends.
    #[test]
    fn cursor_forward_default_count_is_one() {
        let mut model = ScreenModel::default();
        feed(&mut model, b"XYZ\r");
        model.lines[0].dirty = false;

        feed(&mut model, b"\x1b[C");

        assert_eq!(model.lines[0].chars[0], ' ');
        assert_eq!(model.lines[0].chars[1], 'Y');
        assert!(model.lines[0].dirty);
        assert_eq!(model.cursor_x, 1);
    }

    #[test]
    fn cursor_forward_clamps_to_line_width() {
        let mut model = ScreenModel::default();
        let cols = model.width() as usize;
        feed(&mut model, format!("\x1b[{}C", cols + 50).as_bytes());
        assert_eq!(model.cursor_x, cols - 1);
    }

    // Cursor Backward is left at strict ECMA-48 semantics (content-preserving) since
    // the report found no evidence Ink relies on CSI D the same way it relies on
    // CSI C, and blindly blanking on backward movement would be more likely to erase
    // legitimately-preserved content (e.g. cursor repositioning before a redraw).
    #[test]
    fn cursor_backward_does_not_touch_cell_content() {
        let mut model = ScreenModel::default();
        feed(&mut model, b"AB");
        model.lines[0].dirty = false;

        feed(&mut model, b"\x1b[2D");

        assert_eq!(model.lines[0].chars[0], 'A');
        assert_eq!(model.lines[0].chars[1], 'B');
        assert!(!model.lines[0].dirty);
        assert_eq!(model.cursor_x, 0);
    }

    #[test]
    fn erase_in_line_still_blanks_and_marks_dirty() {
        let mut model = ScreenModel::default();
        feed(&mut model, b"ABCDE\r");
        model.lines[0].dirty = false;

        feed(&mut model, b"\x1b[2K");

        assert!(model.lines[0].chars.iter().all(|&c| c == ' '));
        assert!(model.lines[0].dirty);
    }
}
