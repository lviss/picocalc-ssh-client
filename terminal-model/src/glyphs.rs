use embedded_graphics::Pixel;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::*;

/// Box-drawing block (U+2500-U+259F): already hand-drawn via vector primitives
/// instead of the `profont` bitmap font, which doesn't cover this range.
pub fn is_box_char(c: char) -> bool {
    ('\u{2500}'..='\u{259F}').contains(&c)
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

    #[test]
    fn box_drawing_range_is_recognized() {
        assert!(is_box_char('\u{2500}'));
        assert!(is_box_char('\u{259F}'));
        assert!(!is_box_char('A'));
        assert!(!is_box_char(' '));
    }

    #[test]
    fn draw_box_char_actually_paints_pixels() {
        for c in [
            '\u{2500}', '\u{2502}', '\u{250C}', '\u{2510}', '\u{2514}', '\u{2518}', '\u{251C}',
            '\u{2524}', '\u{252C}', '\u{2534}', '\u{253C}', '\u{2501}', '\u{2503}', '\u{2588}',
            '\u{2580}', '\u{2584}', '\u{2591}', '\u{2592}', '\u{2593}',
        ] {
            let mut display: MockDisplay<Rgb565> = MockDisplay::new();
            display.set_allow_out_of_bounds_drawing(true);
            display.set_allow_overdraw(true);
            draw_box_char(&mut display, c, 0, 0, 10, 20, Rgb565::WHITE);
            assert_ne!(
                display,
                MockDisplay::new(),
                "U+{:04X} produced no pixels",
                c as u32
            );
        }
    }
}
