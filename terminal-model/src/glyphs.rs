use embedded_graphics::Pixel;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::*;

/// Box-drawing block (U+2500-U+259F): already hand-drawn via vector primitives
/// instead of the `profont` bitmap font, which doesn't cover this range.
pub fn is_box_char(c: char) -> bool {
    ('\u{2500}'..='\u{259F}').contains(&c)
}

/// Decorative Unicode symbols claude-code's Ink-based TUI emits for its prompt
/// chrome (chevron, bullets, ellipsis, arrows, spinner/status glyphs). `profont`
/// only covers ASCII + Latin-1, so these fall back to its literal `?` glyph unless
/// hand-drawn here, the same way box-drawing characters already are.
pub fn is_symbol_char(c: char) -> bool {
    matches!(
        c,
        '\u{276F}' // ❯ prompt chevron
            | '\u{2022}' // • bullet
            | '\u{25CF}' // ● black circle
            | '\u{25CB}' // ○ white circle
            | '\u{2026}' // … ellipsis
            | '\u{2190}' // ← left arrow
            | '\u{2191}' // ↑ up arrow
            | '\u{2192}' // → right arrow
            | '\u{2193}' // ↓ down arrow
            | '\u{2733}' // ✳ eight spoked asterisk
    )
}

pub fn draw_box_char<D>(display: &mut D, c: char, x: i32, y: i32, w: u32, h: u32, color: Rgb565)
where
    D: DrawTarget<Color = Rgb565>,
{
    let cx = x + (w / 2) as i32;
    let cy = y + (h / 2) as i32;
    let stroke = 1; // Line thickness

    // Helper to draw line
    let line = |display: &mut D, x0, y0, x1, y1| {
        Line::new(Point::new(x0, y0), Point::new(x1, y1))
            .into_styled(PrimitiveStyle::with_stroke(color, stroke))
            .draw(display)
            .ok();
    };

    match c {
        // Light horizontal
        '\u{2500}' => line(display, x, cy, x + w as i32, cy),
        // Light vertical
        '\u{2502}' => line(display, cx, y, cx, y + h as i32),
        // Light down and right
        '\u{250C}' => {
            line(display, cx, cy, x + w as i32, cy);
            line(display, cx, cy, cx, y + h as i32);
        }
        // Light down and left
        '\u{2510}' => {
            line(display, x, cy, cx, cy);
            line(display, cx, cy, cx, y + h as i32);
        }
        // Light up and right
        '\u{2514}' => {
            line(display, cx, cy, x + w as i32, cy);
            line(display, cx, y, cx, cy);
        }
        // Light up and left
        '\u{2518}' => {
            line(display, x, cy, cx, cy);
            line(display, cx, y, cx, cy);
        }
        // Light vertical and right
        '\u{251C}' => {
            line(display, cx, y, cx, y + h as i32);
            line(display, cx, cy, x + w as i32, cy);
        }
        // Light vertical and left
        '\u{2524}' => {
            line(display, cx, y, cx, y + h as i32);
            line(display, x, cy, cx, cy);
        }
        // Light horizontal and down
        '\u{252C}' => {
            line(display, x, cy, x + w as i32, cy);
            line(display, cx, cy, cx, y + h as i32);
        }
        // Light horizontal and up
        '\u{2534}' => {
            line(display, x, cy, x + w as i32, cy);
            line(display, cx, y, cx, cy);
        }
        // Light vertical and horizontal
        '\u{253C}' => {
            line(display, x, cy, x + w as i32, cy);
            line(display, cx, y, cx, y + h as i32);
        }
        // Heavy horizontal
        '\u{2501}' => {
            Line::new(Point::new(x, cy), Point::new(x + w as i32, cy))
                .into_styled(PrimitiveStyle::with_stroke(color, 2))
                .draw(display)
                .ok();
        }
        // Heavy vertical
        '\u{2503}' => {
            Line::new(Point::new(cx, y), Point::new(cx, y + h as i32))
                .into_styled(PrimitiveStyle::with_stroke(color, 2))
                .draw(display)
                .ok();
        }
        // Block
        '\u{2588}' => {
            display
                .fill_solid(&Rectangle::new(Point::new(x, y), Size::new(w, h)), color)
                .ok();
        }
        // Upper half block
        '\u{2580}' => {
            display
                .fill_solid(
                    &Rectangle::new(Point::new(x, y), Size::new(w, h / 2)),
                    color,
                )
                .ok();
        }
        // Lower half block
        '\u{2584}' => {
            display
                .fill_solid(
                    &Rectangle::new(Point::new(x, y + (h / 2) as i32), Size::new(w, h - h / 2)),
                    color,
                )
                .ok();
        }
        // Shades
        '\u{2591}' => draw_shade(display, x, y, w, h, color, 1),
        '\u{2592}' => draw_shade(display, x, y, w, h, color, 2),
        '\u{2593}' => draw_shade(display, x, y, w, h, color, 3),

        // Rounded corners
        '\u{256D}' => {
            // Top-left
            Arc::new(
                Point::new(x + w as i32 / 2, y + h as i32 / 2),
                w,
                Angle::from_degrees(180.0),
                Angle::from_degrees(90.0),
            )
            .into_styled(PrimitiveStyle::with_stroke(color, stroke))
            .draw(display)
            .ok();
            line(display, cx, cy + h as i32 / 2, cx, y + h as i32); // Extend down
            line(display, cx + w as i32 / 2, cy, x + w as i32, cy); // Extend right
        }
        '\u{256E}' => {
            // Top-right
            Arc::new(
                Point::new(x - w as i32 / 2, y + h as i32 / 2),
                w,
                Angle::from_degrees(270.0),
                Angle::from_degrees(90.0),
            )
            .into_styled(PrimitiveStyle::with_stroke(color, stroke))
            .draw(display)
            .ok();
            line(display, cx, cy + h as i32 / 2, cx, y + h as i32); // Extend down
            line(display, x, cy, cx - w as i32 / 2, cy); // Extend left
        }
        '\u{2570}' => {
            // Bottom-left
            Arc::new(
                Point::new(x + w as i32 / 2, y - h as i32 / 2),
                w,
                Angle::from_degrees(90.0),
                Angle::from_degrees(90.0),
            )
            .into_styled(PrimitiveStyle::with_stroke(color, stroke))
            .draw(display)
            .ok();
            line(display, cx, y, cx, cy - h as i32 / 2); // Extend up
            line(display, cx + w as i32 / 2, cy, x + w as i32, cy); // Extend right
        }
        '\u{256F}' => {
            // Bottom-right
            Arc::new(
                Point::new(x - w as i32 / 2, y - h as i32 / 2),
                w,
                Angle::from_degrees(0.0),
                Angle::from_degrees(90.0),
            )
            .into_styled(PrimitiveStyle::with_stroke(color, stroke))
            .draw(display)
            .ok();
            line(display, cx, y, cx, cy - h as i32 / 2); // Extend up
            line(display, x, cy, cx - w as i32 / 2, cy); // Extend left
        }

        // Double lines
        '\u{2550}' => {
            // Horizontal double
            Line::new(Point::new(x, cy - 1), Point::new(x + w as i32, cy - 1))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(x, cy + 1), Point::new(x + w as i32, cy + 1))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
        }
        '\u{2551}' => {
            // Vertical double
            Line::new(Point::new(cx - 1, y), Point::new(cx - 1, y + h as i32))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(cx + 1, y), Point::new(cx + 1, y + h as i32))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
        }
        // Double corners (simplified as single heavy for now to save space/complexity, or proper implementation)
        '\u{2554}' => {
            // Double down-right
            Line::new(Point::new(cx - 1, cy), Point::new(cx - 1, y + h as i32))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(cx + 1, cy), Point::new(cx + 1, y + h as i32))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(cx, cy - 1), Point::new(x + w as i32, cy - 1))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(cx, cy + 1), Point::new(x + w as i32, cy + 1))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
        }
        '\u{2557}' => {
            // Double down-left
            Line::new(Point::new(cx - 1, cy), Point::new(cx - 1, y + h as i32))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(cx + 1, cy), Point::new(cx + 1, y + h as i32))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(x, cy - 1), Point::new(cx, cy - 1))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(x, cy + 1), Point::new(cx, cy + 1))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
        }
        '\u{255A}' => {
            // Double up-right
            Line::new(Point::new(cx - 1, y), Point::new(cx - 1, cy))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(cx + 1, y), Point::new(cx + 1, cy))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(cx, cy - 1), Point::new(x + w as i32, cy - 1))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(cx, cy + 1), Point::new(x + w as i32, cy + 1))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
        }
        '\u{255D}' => {
            // Double up-left
            Line::new(Point::new(cx - 1, y), Point::new(cx - 1, cy))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(cx + 1, y), Point::new(cx + 1, cy))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(x, cy - 1), Point::new(cx, cy - 1))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
            Line::new(Point::new(x, cy + 1), Point::new(cx, cy + 1))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
        }

        _ => {
            // Fallback for unhandled box chars: draw a small rectangle
            Rectangle::new(Point::new(x + 2, y + 2), Size::new(w - 4, h - 4))
                .into_styled(PrimitiveStyle::with_stroke(color, 1))
                .draw(display)
                .ok();
        }
    }
}

/// Vector-draws the decorative symbols `is_symbol_char` matches, the same way
/// `draw_box_char` already hand-draws the box-drawing block, so these codepoints
/// never fall through to `Text::draw()` and hit `profont`'s `?` fallback glyph.
pub fn draw_symbol_char<D>(display: &mut D, c: char, x: i32, y: i32, w: u32, h: u32, color: Rgb565)
where
    D: DrawTarget<Color = Rgb565>,
{
    let cx = x + (w / 2) as i32;
    let cy = y + (h / 2) as i32;

    let line = |display: &mut D, x0, y0, x1, y1, stroke| {
        Line::new(Point::new(x0, y0), Point::new(x1, y1))
            .into_styled(PrimitiveStyle::with_stroke(color, stroke))
            .draw(display)
            .ok();
    };

    let filled_circle = |display: &mut D, diameter: u32| {
        let d = diameter.max(2);
        let top_left = Point::new(cx - (d / 2) as i32, cy - (d / 2) as i32);
        Circle::new(top_left, d)
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(display)
            .ok();
    };

    let outline_circle = |display: &mut D, diameter: u32| {
        let d = diameter.max(2);
        let top_left = Point::new(cx - (d / 2) as i32, cy - (d / 2) as i32);
        Circle::new(top_left, d)
            .into_styled(PrimitiveStyle::with_stroke(color, 1))
            .draw(display)
            .ok();
    };

    match c {
        // ❯ prompt chevron: a right-pointing angle bracket
        '\u{276F}' => {
            let x0 = x + w as i32 / 4;
            let x1 = x + (w as i32 * 3) / 4;
            let y0 = y + h as i32 / 4;
            let y1 = y + (h as i32 * 3) / 4;
            line(display, x0, y0, x1, cy, 2);
            line(display, x1, cy, x0, y1, 2);
        }
        // • bullet: small filled circle
        '\u{2022}' => filled_circle(display, h / 3),
        // ● black circle: larger filled circle
        '\u{25CF}' => filled_circle(display, (h * 2) / 3),
        // ○ white circle: outline only
        '\u{25CB}' => outline_circle(display, (h * 2) / 3),
        // … ellipsis: three evenly spaced dots along the baseline
        '\u{2026}' => {
            let dot_y = y + (h as i32 * 4) / 5;
            for i in 0..3i32 {
                let dot_x = x + (w as i32 * (2 * i + 1)) / 6;
                Pixel(Point::new(dot_x, dot_y), color).draw(display).ok();
                Pixel(Point::new(dot_x + 1, dot_y), color)
                    .draw(display)
                    .ok();
            }
        }
        // ← → ↑ ↓ arrows: a shaft plus a small arrowhead
        '\u{2190}' => {
            line(display, x + w as i32 - 2, cy, x + 2, cy, 1);
            line(
                display,
                x + 2,
                cy,
                x + 2 + w as i32 / 3,
                cy - h as i32 / 4,
                1,
            );
            line(
                display,
                x + 2,
                cy,
                x + 2 + w as i32 / 3,
                cy + h as i32 / 4,
                1,
            );
        }
        '\u{2192}' => {
            line(display, x + 2, cy, x + w as i32 - 2, cy, 1);
            line(
                display,
                x + w as i32 - 2,
                cy,
                x + w as i32 - 2 - w as i32 / 3,
                cy - h as i32 / 4,
                1,
            );
            line(
                display,
                x + w as i32 - 2,
                cy,
                x + w as i32 - 2 - w as i32 / 3,
                cy + h as i32 / 4,
                1,
            );
        }
        '\u{2191}' => {
            line(display, cx, y + h as i32 - 2, cx, y + 2, 1);
            line(
                display,
                cx,
                y + 2,
                cx - w as i32 / 4,
                y + 2 + h as i32 / 3,
                1,
            );
            line(
                display,
                cx,
                y + 2,
                cx + w as i32 / 4,
                y + 2 + h as i32 / 3,
                1,
            );
        }
        '\u{2193}' => {
            line(display, cx, y + 2, cx, y + h as i32 - 2, 1);
            line(
                display,
                cx,
                y + h as i32 - 2,
                cx - w as i32 / 4,
                y + h as i32 - 2 - h as i32 / 3,
                1,
            );
            line(
                display,
                cx,
                y + h as i32 - 2,
                cx + w as i32 / 4,
                y + h as i32 - 2 - h as i32 / 3,
                1,
            );
        }
        // ✳ eight spoked asterisk: burst of lines through the center
        '\u{2733}' => {
            line(display, x + 1, cy, x + w as i32 - 1, cy, 1);
            line(display, cx, y + 1, cx, y + h as i32 - 1, 1);
            line(display, x + 2, y + 2, x + w as i32 - 2, y + h as i32 - 2, 1);
            line(display, x + w as i32 - 2, y + 2, x + 2, y + h as i32 - 2, 1);
        }
        _ => {}
    }
}

fn draw_shade<D>(display: &mut D, x: i32, y: i32, w: u32, h: u32, color: Rgb565, density: u8)
where
    D: DrawTarget<Color = Rgb565>,
{
    for py in 0..h {
        for px in 0..w {
            let on = match density {
                1 => (px % 2 == 0) && (py % 2 == 0),    // 25%
                2 => (px + py) % 2 == 0,                // 50%
                3 => !((px % 2 == 0) && (py % 2 == 0)), // 75%
                _ => false,
            };
            if on {
                Pixel(Point::new(x + px as i32, y + py as i32), color)
                    .draw(display)
                    .ok();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::mock_display::MockDisplay;

    // Every codepoint the report captured live from claude-code must dispatch to
    // the vector-draw path rather than falling through to Text::draw()'s profont
    // rendering, which is what previously produced the literal `?` fallback glyph.
    #[test]
    fn report_captured_codepoints_are_symbol_chars() {
        for c in [
            '\u{276F}', '\u{2022}', '\u{2026}', '\u{2190}', '\u{25CF}', '\u{2733}',
        ] {
            assert!(
                is_symbol_char(c),
                "U+{:04X} should dispatch to draw_symbol_char",
                c as u32
            );
        }
    }

    #[test]
    fn plain_ascii_and_box_chars_are_not_symbol_chars() {
        assert!(!is_symbol_char('>'));
        assert!(!is_symbol_char('A'));
        assert!(!is_symbol_char(' '));
        assert!(
            !is_symbol_char('\u{2500}'),
            "box-drawing chars must stay on the draw_box_char path"
        );
        assert!(
            !is_box_char('\u{276F}'),
            "symbol chars must not collide with the box-drawing dispatch"
        );
    }

    #[test]
    fn draw_symbol_char_actually_paints_pixels() {
        // Regression guard for a no-op dispatch: every codepoint in is_symbol_char
        // must produce at least one non-background pixel when drawn, otherwise the
        // fix would silently render nothing instead of fixing the '?' fallback.
        for c in [
            '\u{276F}', '\u{2022}', '\u{25CF}', '\u{25CB}', '\u{2026}', '\u{2190}', '\u{2191}',
            '\u{2192}', '\u{2193}', '\u{2733}',
        ] {
            let mut display: MockDisplay<Rgb565> = MockDisplay::new();
            display.set_allow_out_of_bounds_drawing(true);
            // Strokes intentionally cross near the glyph center (e.g. arrowheads,
            // the asterisk spokes), which redraws some pixels more than once.
            display.set_allow_overdraw(true);
            draw_symbol_char(&mut display, c, 0, 0, 10, 20, Rgb565::WHITE);
            assert_ne!(
                display,
                MockDisplay::new(),
                "U+{:04X} produced no pixels",
                c as u32
            );
        }
    }
}
