//! Interactive Fan Curve canvas widget.
//!
//! Enables dragging control points on a (Temperature °C vs Fan Speed %) graph.
//! Enforces monotonic non-decreasing constraints (`speed[i] >= speed[i-1]`).

use iced::mouse::{self, Cursor};
use iced::widget::canvas::{self, Canvas, Geometry, Path, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurvePoint {
    pub temp_c: f32,
    pub speed_pct: f32,
}

#[derive(Debug, Clone)]
pub enum FanCurveMessage {
    PointChanged(usize, f32, f32),
}

#[derive(Default)]
pub struct FanCurveState {
    dragged_idx: Option<usize>,
}

pub struct FanCurveCanvas {
    pub points: Vec<CurvePoint>,
}

impl FanCurveCanvas {
    pub const MIN_TEMP: f32 = 30.0;
    pub const MAX_TEMP: f32 = 100.0;
    pub const MIN_SPEED: f32 = 0.0;
    pub const MAX_SPEED: f32 = 100.0;

    pub fn new(points: Vec<CurvePoint>) -> Self {
        Self { points }
    }

    pub fn view<'a>(self) -> Element<'a, FanCurveMessage> {
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(280.0))
            .into()
    }

    fn to_screen(&self, point: &CurvePoint, bounds: Rectangle) -> Point {
        let x_norm = (point.temp_c - Self::MIN_TEMP) / (Self::MAX_TEMP - Self::MIN_TEMP);
        let y_norm = (point.speed_pct - Self::MIN_SPEED) / (Self::MAX_SPEED - Self::MIN_SPEED);

        let margin = 30.0;
        let plot_w = bounds.width - margin * 2.0;
        let plot_h = bounds.height - margin * 2.0;

        Point {
            x: bounds.x + margin + x_norm * plot_w,
            y: bounds.y + margin + (1.0 - y_norm) * plot_h,
        }
    }

    fn screen_to_curve(p: Point, bounds: Rectangle) -> (f32, f32) {
        let margin = 30.0;
        let plot_w = bounds.width - margin * 2.0;
        let plot_h = bounds.height - margin * 2.0;

        let x_norm = ((p.x - bounds.x - margin) / plot_w).clamp(0.0, 1.0);
        let y_norm = (1.0 - (p.y - bounds.y - margin) / plot_h).clamp(0.0, 1.0);

        let temp = Self::MIN_TEMP + x_norm * (Self::MAX_TEMP - Self::MIN_TEMP);
        let speed = Self::MIN_SPEED + y_norm * (Self::MAX_SPEED - Self::MIN_SPEED);

        (temp.round(), speed.round())
    }
}

impl canvas::Program<FanCurveMessage> for FanCurveCanvas {
    type State = FanCurveState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: Cursor,
    ) -> (canvas::event::Status, Option<FanCurveMessage>) {
        let cursor_pos = match cursor.position_in(bounds) {
            Some(pos) => Point {
                x: pos.x + bounds.x,
                y: pos.y + bounds.y,
            },
            None => return (canvas::event::Status::Ignored, None),
        };

        match event {
            canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                // Find closest point within grab radius
                for (idx, p) in self.points.iter().enumerate() {
                    let screen_pt = self.to_screen(p, bounds);
                    let dist = ((screen_pt.x - cursor_pos.x).powi(2)
                        + (screen_pt.y - cursor_pos.y).powi(2))
                    .sqrt();
                    if dist <= 14.0 {
                        state.dragged_idx = Some(idx);
                        return (canvas::event::Status::Captured, None);
                    }
                }
                (canvas::event::Status::Ignored, None)
            }
            canvas::Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                if state.dragged_idx.is_some() {
                    state.dragged_idx = None;
                    return (canvas::event::Status::Captured, None);
                }
                (canvas::event::Status::Ignored, None)
            }
            canvas::Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                if let Some(idx) = state.dragged_idx {
                    let (new_temp, mut new_speed) = Self::screen_to_curve(cursor_pos, bounds);

                    // Monotonic constraint: speed cannot be less than previous or greater than next
                    if idx > 0 {
                        new_speed = new_speed.max(self.points[idx - 1].speed_pct);
                    }
                    if idx + 1 < self.points.len() {
                        new_speed = new_speed.min(self.points[idx + 1].speed_pct);
                    }

                    return (
                        canvas::event::Status::Captured,
                        Some(FanCurveMessage::PointChanged(idx, new_temp, new_speed)),
                    );
                }
                (canvas::event::Status::Ignored, None)
            }
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &Renderer,
        _theme: &Theme,
        bounds: Rectangle,
        _cursor: Cursor,
    ) -> Vec<Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        let margin = 30.0;
        let plot_rect = Rectangle {
            x: margin,
            y: margin,
            width: bounds.width - margin * 2.0,
            height: bounds.height - margin * 2.0,
        };

        // Background
        frame.fill_rectangle(
            Point {
                x: plot_rect.x,
                y: plot_rect.y,
            },
            plot_rect.size(),
            Color::from_rgba(0.15, 0.15, 0.15, 0.8),
        );

        // Grid lines (every 20% / 20°C)
        let grid_stroke = Stroke::default()
            .with_color(Color::from_rgba(0.3, 0.3, 0.3, 0.5))
            .with_width(1.0);

        for step in 1..5 {
            let y = plot_rect.y + (step as f32 / 5.0) * plot_rect.height;
            frame.stroke(
                &Path::line(
                    Point { x: plot_rect.x, y },
                    Point {
                        x: plot_rect.x + plot_rect.width,
                        y,
                    },
                ),
                grid_stroke,
            );
        }

        // Draw curve lines
        if self.points.len() >= 2 {
            let mut path_builder = canvas::path::Builder::new();
            let first = self.to_screen(&self.points[0], bounds);
            path_builder.move_to(Point {
                x: first.x - bounds.x,
                y: first.y - bounds.y,
            });

            for p in &self.points[1..] {
                let sp = self.to_screen(p, bounds);
                path_builder.line_to(Point {
                    x: sp.x - bounds.x,
                    y: sp.y - bounds.y,
                });
            }

            let path = path_builder.build();
            frame.stroke(
                &path,
                Stroke::default()
                    .with_color(Color::from_rgb(0.0, 0.47, 0.83))
                    .with_width(3.0),
            );
        }

        // Draw point handles
        for p in &self.points {
            let sp = self.to_screen(p, bounds);
            let center = Point {
                x: sp.x - bounds.x,
                y: sp.y - bounds.y,
            };

            // Outer ring
            frame.fill(&Path::circle(center, 7.0), Color::from_rgb(0.0, 0.47, 0.83));
            // Inner white dot
            frame.fill(&Path::circle(center, 3.5), Color::WHITE);
        }

        vec![frame.into_geometry()]
    }
}
