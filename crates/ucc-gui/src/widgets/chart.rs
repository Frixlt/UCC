//! Live Telemetry history chart widget.
//!
//! Renders rolling multi-channel time-series data over an horizon (e.g. 60 seconds).

use iced::mouse::Cursor;
use iced::widget::canvas::{self, Canvas, Geometry, Path, Stroke};
use iced::{Color, Element, Length, Point, Rectangle, Renderer, Theme};

#[derive(Debug, Clone, Default)]
pub struct TelemetryDataPoint {
    pub cpu_temp: f32,
    pub gpu_temp: f32,
    pub fan_rpm: f32,
}

pub struct TelemetryChartCanvas {
    pub history: Vec<TelemetryDataPoint>,
    pub max_samples: usize,
}

impl TelemetryChartCanvas {
    pub fn new(history: Vec<TelemetryDataPoint>, max_samples: usize) -> Self {
        Self {
            history,
            max_samples: max_samples.max(10),
        }
    }

    pub fn view<'a, Message: 'a>(self) -> Element<'a, Message> {
        Canvas::new(self)
            .width(Length::Fill)
            .height(Length::Fixed(200.0))
            .into()
    }
}

impl<Message> canvas::Program<Message> for TelemetryChartCanvas {
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

        let margin = 20.0;
        let plot_rect = Rectangle {
            x: margin,
            y: margin,
            width: (bounds.width - margin * 2.0).max(10.0),
            height: (bounds.height - margin * 2.0).max(10.0),
        };

        // Background
        frame.fill_rectangle(
            Point {
                x: plot_rect.x,
                y: plot_rect.y,
            },
            plot_rect.size(),
            Color::from_rgba(0.12, 0.12, 0.12, 0.9),
        );

        // Grid lines (every 25%)
        let grid_stroke = Stroke::default()
            .with_color(Color::from_rgba(0.25, 0.25, 0.25, 0.4))
            .with_width(1.0);

        for step in 1..4 {
            let y = plot_rect.y + (step as f32 / 4.0) * plot_rect.height;
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

        if self.history.len() < 2 {
            return vec![frame.into_geometry()];
        }

        let _n = self.history.len();
        let dx = plot_rect.width / (self.max_samples as f32);

        // Draw CPU/GPU and Fan lines
        let mut cpu_path = canvas::path::Builder::new();
        let mut gpu_path = canvas::path::Builder::new();
        let mut fan_path = canvas::path::Builder::new();

        for (i, pt) in self.history.iter().enumerate() {
            let x = plot_rect.x + (i as f32) * dx;

            // Normalize 0..100°C to plot height
            let cpu_y =
                plot_rect.y + (1.0 - (pt.cpu_temp / 100.0).clamp(0.0, 1.0)) * plot_rect.height;
            let gpu_y =
                plot_rect.y + (1.0 - (pt.gpu_temp / 100.0).clamp(0.0, 1.0)) * plot_rect.height;
            let fan_y =
                plot_rect.y + (1.0 - (pt.fan_rpm / 4000.0).clamp(0.0, 1.0)) * plot_rect.height;

            if i == 0 {
                cpu_path.move_to(Point { x, y: cpu_y });
                gpu_path.move_to(Point { x, y: gpu_y });
                fan_path.move_to(Point { x, y: fan_y });
            } else {
                cpu_path.line_to(Point { x, y: cpu_y });
                gpu_path.line_to(Point { x, y: gpu_y });
                fan_path.line_to(Point { x, y: fan_y });
            }
        }

        // Stroke CPU (coral/red)
        frame.stroke(
            &cpu_path.build(),
            Stroke::default()
                .with_color(Color::from_rgb(0.95, 0.35, 0.25))
                .with_width(2.0),
        );

        // Stroke GPU (green)
        frame.stroke(
            &gpu_path.build(),
            Stroke::default()
                .with_color(Color::from_rgb(0.25, 0.85, 0.35))
                .with_width(2.0),
        );

        // Stroke Fan (cyan)
        frame.stroke(
            &fan_path.build(),
            Stroke::default()
                .with_color(Color::from_rgb(0.2, 0.7, 0.95))
                .with_width(1.5),
        );

        vec![frame.into_geometry()]
    }
}
