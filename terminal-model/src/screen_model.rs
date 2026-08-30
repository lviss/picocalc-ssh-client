use embedded_graphics::mono_font::MonoFont;
use embedded_graphics::pixelcolor::{Rgb565, Rgb888};
use embedded_graphics::prelude::*;
use vte::Perform;

use alloc::string::String;
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
        for c in self.chars.iter_mut() {
            *c = ' ';
        }
        for a in self.attrs.iter_mut() {
            *a = Attrs::default();
        }
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
    /// Transient text shown on top of the terminal contents (e.g. the battery
    /// overlay). Never mutates `lines`/`scrollback` - the painter composites it
    /// over the framebuffer each frame while set, and `clear_overlay` forces a
    /// full repaint from the untouched cell data so dismissal is invisible.
    pub overlay: Option<String>,
}

impl Default for ScreenModel {
    fn default() -> Self {
        let font = FONTS[2];
        let cols =
            ((SCREEN_WIDTH as u32) / (font.character_size.width + font.character_spacing)) as usize;
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
            overlay: None,
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

    /// Show `text` as an overlay. Purely a paint-time overlay flag - the cell
    /// buffer is never touched, so any host output that lands underneath while
    /// it's showing is preserved untouched.
    pub fn show_overlay(&mut self, text: String) {
        self.overlay = Some(text);
    }

    /// Hide the overlay and force a full repaint so the real, possibly-changed
    /// cell contents underneath it are redrawn exactly as they'd look had the
    /// overlay never been shown.
    pub fn clear_overlay(&mut self) {
        if self.overlay.take().is_some() {
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
            b'\n' => {
                // LF
                self.cursor_y += 1;
                if self.cursor_y >= self.rows {
                    self.scroll_up();
                    self.cursor_y = self.rows - 1;
                }
            }
            b'\r' => {
                // CR
                self.cursor_x = 0;
            }
            // BS
            b'\x08' if self.cursor_x > 0 => {
                self.cursor_x -= 1;
            }
            _ => {}
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        if ignore || !intermediates.is_empty() {
            return;
        }

        match action {
            'A' => {
                // Cursor Up
                let n = cursor_move_count(params);
                self.cursor_y = self.cursor_y.saturating_sub(n);
            }
            'B' => {
                // Cursor Down
                let n = cursor_move_count(params);
                self.cursor_y = (self.cursor_y + n).min(self.rows - 1);
            }
            'C' => {
                // Cursor Forward
                let n = cursor_move_count(params);
                self.cursor_x = (self.cursor_x + n).min(self.cols - 1);
            }
            'D' => {
                // Cursor Backward
                let n = cursor_move_count(params);
                self.cursor_x = self.cursor_x.saturating_sub(n);
            }
            'H' | 'f' => {
                // Cursor Position
                let mut iter = params.iter();
                let row = iter.next().map(|p| p[0]).unwrap_or(1).max(1) as usize - 1;
                let col = iter.next().map(|p| p[0]).unwrap_or(1).max(1) as usize - 1;
                self.cursor_y = row.min(self.rows - 1);
                self.cursor_x = col.min(self.cols - 1);
            }
            'd' => {
                // Line Position Absolute (VPA): move to row Pn, column unchanged.
                let row = params.iter().next().map(|p| p[0]).unwrap_or(1).max(1) as usize - 1;
                self.cursor_y = row.min(self.rows - 1);
            }
            'J' => {
                // Erase in Display
                let n = params.iter().next().map(|p| p[0]).unwrap_or(0);
                match n {
                    0 => {
                        // Cursor to end
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
                    1 => {
                        // Beginning to cursor
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
                    2 => {
                        // Entire screen
                        self.clear();
                    }
                    _ => {}
                }
            }
            'K' => {
                // Erase in Line
                let n = params.iter().next().map(|p| p[0]).unwrap_or(0);
                let line = &mut self.lines[self.cursor_y];
                match n {
                    0 => {
                        // Cursor to end
                        for i in self.cursor_x..self.cols {
                            line.chars[i] = ' ';
                            line.attrs[i] = self.current_attrs;
                        }
                    }
                    1 => {
                        // Beginning to cursor
                        for i in 0..=self.cursor_x {
                            line.chars[i] = ' ';
                            line.attrs[i] = self.current_attrs;
                        }
                    }
                    2 => {
                        // Entire line
                        for i in 0..self.cols {
                            line.chars[i] = ' ';
                            line.attrs[i] = self.current_attrs;
                        }
                    }
                    _ => {}
                }
                line.dirty = true;
            }
            'm' => {
                // SGR
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

    fn hook(&mut self, _params: &vte::Params, _intermediates: &[u8], _ignore: bool, _action: char) {
    }
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

    // Cursor Forward and Cursor Backward are both content-preserving, strict ECMA-48
    // semantics: CUF/CUB must move the cursor without touching cell content.
    #[test]
    fn cursor_forward_and_backward_do_not_touch_cell_content() {
        let mut model = ScreenModel::default();
        feed(&mut model, b"AB");
        model.lines[0].dirty = false;

        feed(&mut model, b"\x1b[2D");
        assert_eq!(model.lines[0].chars[0], 'A');
        assert_eq!(model.lines[0].chars[1], 'B');
        assert!(!model.lines[0].dirty);
        assert_eq!(model.cursor_x, 0);

        feed(&mut model, b"\x1b[2C");
        assert_eq!(model.lines[0].chars[0], 'A');
        assert_eq!(model.lines[0].chars[1], 'B');
        assert!(!model.lines[0].dirty);
        assert_eq!(model.cursor_x, 2);
    }

    // Reproduces the regression from /ai/firstmate/data/picocalc-random-blanked-chars/report.md:
    // Ink uses the exact same bare `CSI C` idiom to skip over a column whose character
    // is unchanged from the previous frame during a redraw - including a real mid-word
    // letter - as it does for runs of typed spaces it believes are already blank. This
    // replays the report's own live-captured byte sequence ("Captain, s" + bare CSI C
    // skipping an already-correct 'h' + "ip" + "shape.") and asserts the skipped letter
    // survives untouched, matching content-preserving CUF semantics. Relies on
    // cursor_move_count treating the bare form as count 1, not 0.
    #[test]
    fn cursor_forward_over_unchanged_midword_letter_does_not_blank_it() {
        let mut model = ScreenModel::default();
        feed(&mut model, b"Captain, shipshape.\r");

        feed(&mut model, b"Captain, s\x1b[Cip");

        let line: alloc::string::String = model.lines[0].chars[0..19].iter().collect();
        assert_eq!(line, "Captain, shipshape.");
    }

    // vte always pushes a parameter (0, not absent) even for a bare CSI, so a naive
    // `.unwrap_or(1)` on the parsed value would silently compute n=0 for the extremely
    // common bare `ESC[C` form (no digit) claude-code's Ink renderer actually emits.
    #[test]
    fn cursor_forward_default_count_is_one() {
        let mut model = ScreenModel::default();
        feed(&mut model, b"XYZ\r");

        feed(&mut model, b"\x1b[C");

        assert_eq!(model.cursor_x, 1);
    }

    #[test]
    fn cursor_forward_clamps_to_line_width() {
        let mut model = ScreenModel::default();
        let cols = model.width() as usize;
        feed(&mut model, format!("\x1b[{}C", cols + 50).as_bytes());
        assert_eq!(model.cursor_x, cols - 1);
    }

    // ECMA-48 treats "0" and "absent" as the same "use the default value" case, so an
    // explicit zero parameter ("\x1b[0C"/"\x1b[0D") must move by 1, exactly like the
    // bare form - cursor_move_count handles both the same way.
    #[test]
    fn cursor_forward_and_backward_move_by_one_on_explicit_zero_param() {
        let mut model = ScreenModel::default();
        feed(&mut model, b"AB");
        assert_eq!(model.cursor_x, 2);

        feed(&mut model, b"\x1b[0D");
        assert_eq!(model.cursor_x, 1);

        feed(&mut model, b"\x1b[0C");
        assert_eq!(model.cursor_x, 2);
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

    // Reproduces the regression from
    // /ai/firstmate/data/picocalc-overlapping-text-corruption/report.md: a terminal
    // multiplexer's "CUP-then-cheap-VPA-restore" idiom - an absolute cursor position
    // (e.g. for its own status-line redraw) immediately followed by a bare `CSI Pn d`
    // (Line Position Absolute) to restore just the row, leaving the column as the
    // multiplexer's own redraw left it - previously left cursor_y stale since VPA fell
    // through the catch-all. That caused the next print() to land on the wrong row,
    // smashing unrelated content together mid-word.
    #[test]
    fn vpa_line_position_absolute_restores_cursor_row() {
        let mut model = ScreenModel::default();
        feed(&mut model, b"cong"); // row 0: live line so far
        feed(&mut model, b"\x1b[6;1Hunacknowledged data can be"); // row 5: unrelated content
        feed(&mut model, b"\x1b[6;5H\x1b[1d"); // CUP then VPA-restore idiom
        feed(&mut model, b"estion window begins"); // should land on row 0

        let row0: alloc::string::String = model.lines[0].chars[0..25].iter().collect();
        let row5: alloc::string::String = model.lines[5].chars[0..27].iter().collect();
        assert_eq!(row0.trim_end(), "congestion window begins");
        assert_eq!(row5.trim_end(), "unacknowledged data can be");
    }

    // The overlay (e.g. the battery readout shown on a power-button press) must
    // never touch the cell buffer - it's a paint-time compositing flag only, so
    // showing it can't perturb full_repaint/dirty state used by other logic.
    #[test]
    fn show_overlay_does_not_touch_cell_buffer_or_repaint_state() {
        let mut model = ScreenModel::default();
        feed(&mut model, b"hello\r");
        model.full_repaint = false;
        model.lines[0].dirty = false;
        let before: alloc::string::String = model.lines[0].chars[0..5].iter().collect();

        model.show_overlay(alloc::string::String::from("Battery: 87%"));

        assert_eq!(model.overlay.as_deref(), Some("Battery: 87%"));
        assert!(!model.full_repaint);
        assert!(!model.lines[0].dirty);
        let after: alloc::string::String = model.lines[0].chars[0..5].iter().collect();
        assert_eq!(before, after);
    }

    // Reproduces the core requirement from the battery-overlay feature: content
    // the remote host writes while the overlay is showing must not be lost, and
    // once the overlay is dismissed the screen must reflect that content exactly
    // as if the overlay had never appeared - i.e. dismissal is just "repaint from
    // the (untouched, up to date) cell buffer", not a snapshot/restore.
    #[test]
    fn overlay_composited_then_dismissed_reverts_to_current_host_content() {
        let mut model = ScreenModel::default();
        feed(&mut model, b"before\r");

        model.show_overlay(alloc::string::String::from("Battery: 87%"));

        // Remote host output arrives while the overlay is up.
        feed(&mut model, b"after-overlay-output\r");

        assert!(model.overlay.is_some());
        model.clear_overlay();

        assert!(model.overlay.is_none());
        assert!(model.full_repaint);
        let row0: alloc::string::String = model.lines[0].chars[0..20].iter().collect();
        assert_eq!(row0, "after-overlay-output");
    }

    #[test]
    fn clear_overlay_is_a_noop_when_nothing_is_showing() {
        let mut model = ScreenModel::default();
        model.full_repaint = false;

        model.clear_overlay();

        assert!(model.overlay.is_none());
        assert!(!model.full_repaint);
    }
}
