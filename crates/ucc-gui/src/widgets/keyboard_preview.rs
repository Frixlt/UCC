//! 3-zone keyboard backlight preview widget.

use iced::mouse::Cursor;
use iced::widget::canvas::{self, Canvas, Geometry, Path, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ZoneColor {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Default for ZoneColor {
    fn default() -> Self {
        Self {
            r: 0,
            g: 120,
            b: 212,
        }
    }
}

pub struct KeyboardCanvas {
    pub left: ZoneColor,
    pub middle: ZoneColor,
    pub right: ZoneColor,
    pub brightness_pct: u8,
}

impl KeyboardCanvas {
    pub fn new(left: ZoneColor, middle: ZoneColor, right: ZoneColor, brightness_pct: u8) -> Self {
        Self {
            left,
            middle,
            right,
            brightness_pct,
        }
    }

    pub fn view<'a, Message: 'a>(self) -> Element<'a, Message> {
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(120.0))
            .into()
    }

    fn to_color(zone: ZoneColor, brightness: u8) -> Color {
        let factor = (brightness as f32 / 100.0).clamp(0.0, 1.0);
        Color::from_rgb(
            (zone.r as f32 / 255.0) * factor,
            (zone.g as f32 / 255.0) * factor,
            (zone.b as f32 / 255.0) * factor,
        )
    }
}

impl<Message> canvas::Program<Message> for KeyboardCanvas {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        let pad = 10.0;
        let total_w = bounds.width - pad * 2.0;
        let total_h = bounds.height - pad * 2.0;
        let zone_w = (total_w - pad * 2.0) / 3.0;

        // Base outline
        frame.fill_rectangle(
            Point { x: pad, y: pad },
            iced::Size {
                width: total_w,
                height: total_h,
            },
            Color::from_rgba(0.08, 0.08, 0.08, 0.95),
        );

        let zones = [
            (pad, self.left, "Left"),
            (pad + zone_w + pad, self.middle, "Middle"),
            (pad + (zone_w + pad) * 2.0, self.right, "Right"),
        ];

        for (x_offset, color, _name) in zones {
            let zone_rect = Rectangle {
                x: x_offset,
                y: pad + 5.0,
                width: zone_w,
                height: total_h - 10.0,
            };

            // Glow / fill
            frame.fill_rectangle(
                Point {
                    x: zone_rect.x,
                    y: zone_rect.y,
                },
                zone_rect.size(),
                Self::to_color(color, self.brightness_pct),
            );

            // Border
            frame.stroke(
                &Path::rectangle(
                    Point {
                        x: zone_rect.x,
                        y: zone_rect.y,
                    },
                    zone_rect.size(),
                ),
                Stroke::default()
                    .with_color(Color::from_rgba(0.4, 0.4, 0.4, 0.5))
                    .with_width(1.0),
            );
        }

        vec![frame.into_geometry()]
    }
}
