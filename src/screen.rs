use core::fmt;
use core::ops::{Deref, DerefMut};
use embassy_embedded_hal::shared_bus::blocking::spi::SpiDeviceWithConfig;
use embassy_rp::gpio::Output;
use embassy_rp::peripherals::SPI1;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::lazy_lock::LazyLock;
use embassy_sync::mutex::Mutex as AsyncMutex;
use embassy_time::{Duration, Ticker};
use embedded_graphics::mono_font::MonoTextStyleBuilder;
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::*;
use embedded_graphics::text::Text;
use mipidsi::interface::SpiInterface;
use mipidsi::models::ILI9488Rgb565;

use terminal_model::glyphs::{draw_box_char, is_box_char};
use terminal_model::screen_model::ScreenModel;
pub use terminal_model::screen_model::{SCREEN_HEIGHT, SCREEN_WIDTH};

// Define PicoCalcDisplay here so it can be used in main.rs and here
pub type PicoCalcDisplay<'a> = mipidsi::Display<
    SpiInterface<
        'a,
        SpiDeviceWithConfig<
            'a,
            NoopRawMutex,
            embassy_rp::spi::Spi<'a, SPI1, embassy_rp::spi::Blocking>,
            Output<'a>,
        >,
        Output<'a>,
    >,
    ILI9488Rgb565,
    Output<'a>,
>;

pub static SCREEN: LazyLock<AsyncMutex<CriticalSectionRawMutex, Screen>> =
    LazyLock::new(|| AsyncMutex::new(Screen::new()));

pub struct Screen {
    model: ScreenModel,
    parser: vte::Parser,
}

impl Deref for Screen {
    type Target = ScreenModel;
    fn deref(&self) -> &ScreenModel {
        &self.model
    }
}

impl DerefMut for Screen {
    fn deref_mut(&mut self) -> &mut ScreenModel {
        &mut self.model
    }
}

impl Screen {
    pub fn new() -> Self {
        Self {
            model: ScreenModel::default(),
            parser: vte::Parser::new(),
        }
    }

    pub fn parse_bytes(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.parser.advance(&mut self.model, *byte);
        }
    }

    pub fn print(&mut self, text: &str) {
        self.parse_bytes(text.as_bytes())
    }

    pub fn clear(&mut self) {
        self.model.clear();
    }

    pub fn update_display(&mut self, display: &mut PicoCalcDisplay) {
        update_display(&mut self.model, display);
    }
}

impl fmt::Write for Screen {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.print(s);
        Ok(())
    }
}

fn update_display(model: &mut ScreenModel, display: &mut PicoCalcDisplay) {
    if model.full_repaint {
        display.clear(Rgb565::BLACK).unwrap();
    }

    let font = model.font;
    let cell_width = font.character_size.width + font.character_spacing;
    let cell_height = font.character_size.height;

    for y in 0..model.rows {
        let line_idx = if model.viewport_offset > 0 {
            // Calculate absolute index in history + lines
            // Total lines = scrollback.len() + lines.len() (which is rows)
            // View start = Total lines - rows - viewport_offset
            // Current row abs index = View start + y
            let total_len = model.scrollback.len() + model.rows;
            let view_start = total_len
                .saturating_sub(model.rows)
                .saturating_sub(model.viewport_offset);
            let abs_idx = view_start + y;

            if abs_idx < model.scrollback.len() {
                Some(&mut model.scrollback[abs_idx])
            } else {
                Some(&mut model.lines[abs_idx - model.scrollback.len()])
            }
        } else {
            Some(&mut model.lines[y])
        };

        let line = match line_idx {
            Some(l) => l,
            None => continue,
        };

        if !line.dirty && !model.full_repaint {
            continue;
        }

        let row_y = y as u32 * cell_height as u32;
        if row_y >= SCREEN_HEIGHT as u32 {
            break;
        }

        for (x, (char, attr)) in line.chars.iter().zip(line.attrs.iter()).enumerate() {
            let col_x = x as u32 * cell_width;
            if col_x >= SCREEN_WIDTH as u32 {
                break;
            }

            let mut fg = attr.fg.to_rgb565(false);
            let mut bg = attr.bg.to_rgb565(true);

            if attr.reverse {
                core::mem::swap(&mut fg, &mut bg);
            }

            if attr.bold {
                // Brighten fg?
                if fg == Rgb565::CSS_LIGHT_GRAY {
                    fg = Rgb565::WHITE;
                }
            }

            // Draw background
            display
                .fill_solid(
                    &Rectangle::new(
                        Point::new(col_x as i32, row_y as i32),
                        Size::new(cell_width, cell_height as u32),
                    ),
                    bg,
                )
                .unwrap();

            // Draw text
            if *char != ' ' {
                let style = MonoTextStyleBuilder::new()
                    .font(font)
                    .text_color(fg)
                    .background_color(bg)
                    .build();

                // We need to handle char string
                let mut buf = [0u8; 4];
                let s = char.encode_utf8(&mut buf);

                if is_box_char(*char) {
                    draw_box_char(
                        display,
                        *char,
                        col_x as i32,
                        row_y as i32,
                        cell_width,
                        cell_height as u32,
                        fg,
                    );
                } else {
                    Text::new(
                        s,
                        Point::new(col_x as i32, row_y as i32 + font.baseline as i32),
                        style,
                    )
                    .draw(display)
                    .ok(); // Ignore errors for missing glyphs
                }
            }

            if attr.underline {
                display
                    .fill_solid(
                        &Rectangle::new(
                            Point::new(col_x as i32, (row_y + cell_height as u32 - 1) as i32),
                            Size::new(cell_width, 1),
                        ),
                        fg,
                    )
                    .unwrap();
            }
        }
        line.dirty = false;
    }
    model.full_repaint = false;

    // Draw cursor
    let cx = model.cursor_x as u32 * cell_width;
    let cy = model.cursor_y as u32 * cell_height as u32;
    if cx < SCREEN_WIDTH as u32 && cy < SCREEN_HEIGHT as u32 {
        display
            .fill_solid(
                &Rectangle::new(
                    Point::new(cx as i32, cy as i32),
                    Size::new(cell_width, cell_height as u32),
                ),
                Rgb565::WHITE,
            )
            .ok();
    }
}

#[embassy_executor::task]
pub async fn screen_painter(mut display: PicoCalcDisplay<'static>) {
    display.clear(Rgb565::BLACK).unwrap();
    if let Err(err) = display.set_vertical_scroll_region(0, 0) {
        // log::error!("failed to set_vertical_scroll_region: {err:?}");
    }

    let mut ticker = Ticker::every(Duration::from_millis(200));
    loop {
        SCREEN.get().lock().await.update_display(&mut display);
        ticker.next().await;
    }
}

pub async fn cls_command(_args: &[&str]) {
    SCREEN.get().lock().await.clear();
}
