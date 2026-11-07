//! Frixit Control Center GUI (Iced Fluent prototype)
//!
//! Features:
//! - 10-page Fluent navigation shell
//! - Interactive fan curve canvas with monotonic constraints
//! - Live multi-channel telemetry history chart
//! - 3-zone keyboard backlight canvas preview
//! - Dark/Light theme switching
//! - Safe low-power GPU adapter selection (`WGPU_POWER_PREF=low_power`)
//! - Read-only preview mode support (`--preview`)

mod navigation;
mod theme;
mod widgets;

use iced::widget::{button, column, container, horizontal_space, row, scrollable, text, Space};
use iced::{Alignment, Element, Length, Subscription, Task, Theme as IcedTheme};
use navigation::Page;
use theme::{Palette, ThemeMode};
use widgets::chart::{TelemetryChartCanvas, TelemetryDataPoint};
use widgets::fan_curve::{CurvePoint, FanCurveCanvas, FanCurveMessage};
use widgets::keyboard_preview::{KeyboardCanvas, ZoneColor};

pub fn main() -> iced::Result {
    // GUARD: ensure wgpu selects the low-power (integrated) GPU adapter
    // to strictly prevent waking the discrete NVIDIA GPU from D3cold sleep.
    if std::env::var_os("WGPU_POWER_PREF").is_none() {
        std::env::set_var("WGPU_POWER_PREF", "low_power");
    }

    iced::application(
        ControlCenterApp::title,
        ControlCenterApp::update,
        ControlCenterApp::view,
    )
    .theme(ControlCenterApp::theme)
    .subscription(ControlCenterApp::subscription)
    .window(iced::window::Settings {
        size: iced::Size::new(1024.0, 680.0),
        min_size: Some(iced::Size::new(800.0, 500.0)),
        position: iced::window::Position::Centered,
        ..Default::default()
    })
    .run_with(ControlCenterApp::new)
}

#[derive(Debug, Clone)]
pub enum Message {
    SelectPage(Page),
    ToggleTheme,
    FanCurveEvent(FanCurveMessage),
    SelectProfile(String),
    Tick,
}

pub struct ControlCenterApp {
    current_page: Page,
    theme_mode: ThemeMode,
    is_preview: bool,
    active_profile: String,
    fan_points: Vec<CurvePoint>,
    telemetry_history: Vec<TelemetryDataPoint>,
    left_color: ZoneColor,
    middle_color: ZoneColor,
    right_color: ZoneColor,
    keyboard_brightness: u8,
}

impl ControlCenterApp {
    pub fn new() -> (Self, Task<Message>) {
        let is_preview = std::env::args().any(|arg| arg == "--preview");

        let default_curve = vec![
            CurvePoint {
                temp_c: 35.0,
                speed_pct: 20.0,
            },
            CurvePoint {
                temp_c: 50.0,
                speed_pct: 35.0,
            },
            CurvePoint {
                temp_c: 65.0,
                speed_pct: 55.0,
            },
            CurvePoint {
                temp_c: 80.0,
                speed_pct: 80.0,
            },
            CurvePoint {
                temp_c: 90.0,
                speed_pct: 100.0,
            },
        ];

        // Seed initial telemetry points
        let initial_history = (0..30)
            .map(|i| {
                let f = i as f32;
                TelemetryDataPoint {
                    cpu_temp: 42.0 + (f * 0.2).sin() * 5.0,
                    gpu_temp: 38.0 + (f * 0.15).cos() * 3.0,
                    fan_rpm: 1800.0,
                }
            })
            .collect();

        let app = Self {
            current_page: Page::Overview,
            theme_mode: ThemeMode::Dark,
            is_preview,
            active_profile: "Balanced".to_string(),
            fan_points: default_curve,
            telemetry_history: initial_history,
            left_color: ZoneColor {
                r: 0,
                g: 120,
                b: 212,
            },
            middle_color: ZoneColor {
                r: 0,
                g: 150,
                b: 255,
            },
            right_color: ZoneColor {
                r: 0,
                g: 120,
                b: 212,
            },
            keyboard_brightness: 75,
        };

        (app, Task::none())
    }

    pub fn title(&self) -> String {
        if self.is_preview {
            "Frixit Control Center [PREVIEW - READ ONLY]".to_string()
        } else {
            "Frixit Control Center".to_string()
        }
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::SelectPage(page) => {
                self.current_page = page;
            }
            Message::ToggleTheme => {
                self.theme_mode = match self.theme_mode {
                    ThemeMode::Dark => ThemeMode::Light,
                    ThemeMode::Light => ThemeMode::Dark,
                    ThemeMode::Auto => ThemeMode::Light,
                };
            }
            Message::FanCurveEvent(FanCurveMessage::PointChanged(idx, temp, speed)) => {
                if !self.is_preview && idx < self.fan_points.len() {
                    self.fan_points[idx] = CurvePoint {
                        temp_c: temp,
                        speed_pct: speed,
                    };
                }
            }
            Message::SelectProfile(profile) => {
                if !self.is_preview {
                    self.active_profile = profile;
                }
            }
            Message::Tick => {
                // Rolling telemetry simulation for live charts
                let last = self.telemetry_history.last().cloned().unwrap_or_default();
                let new_pt = TelemetryDataPoint {
                    cpu_temp: (last.cpu_temp + 0.5).min(85.0),
                    gpu_temp: (last.gpu_temp + 0.3).min(80.0),
                    fan_rpm: 2000.0,
                };
                self.telemetry_history.push(new_pt);
                if self.telemetry_history.len() > 60 {
                    self.telemetry_history.remove(0);
                }
            }
        }
    }

    pub fn subscription(&self) -> Subscription<Message> {
        // 1-second UI tick for live telemetry charts
        iced::time::every(std::time::Duration::from_secs(1)).map(|_| Message::Tick)
    }

    pub fn theme(&self) -> IcedTheme {
        match self.theme_mode {
            ThemeMode::Dark => IcedTheme::Dark,
            ThemeMode::Light => IcedTheme::Light,
            ThemeMode::Auto => IcedTheme::Dark,
        }
    }

    pub fn view(&self) -> Element<'_, Message> {
        let palette = Palette::for_mode(self.theme_mode);

        // Sidebar with 10 navigation buttons
        let mut nav_col = column!().spacing(4).padding(8);
        for page in Page::ALL {
            let label = format!("{} {}", page.icon(), page.title());
            let btn = button(text(label).size(13))
                .width(Length::Fill)
                .padding([8, 12])
                .on_press(Message::SelectPage(page));

            nav_col = nav_col.push(btn);
        }

        let sidebar = container(nav_col)
            .width(Length::Fixed(210.0))
            .height(Length::Fill);

        // Top bar
        let top_title = text(format!(
            "{} {}",
            self.current_page.icon(),
            self.current_page.title()
        ))
        .size(20);

        let preview_tag: Element<_> = if self.is_preview {
            container(text("READ-ONLY PREVIEW").size(11))
                .padding([3, 8])
                .into()
        } else {
            Space::with_width(Length::Shrink).into()
        };

        let profile_pill =
            container(text(format!("Profile: {}", self.active_profile)).size(12)).padding([4, 10]);

        let theme_btn = button(
            text(match self.theme_mode {
                ThemeMode::Dark => "🌙 Dark",
                ThemeMode::Light => "☀️ Light",
                ThemeMode::Auto => "⚙️ Auto",
            })
            .size(12),
        )
        .on_press(Message::ToggleTheme);

        let top_bar = row![
            top_title,
            preview_tag,
            horizontal_space(),
            profile_pill,
            theme_btn,
        ]
        .spacing(12)
        .padding(14)
        .align_y(Alignment::Center);

        // Page content router
        let content: Element<_> = match self.current_page {
            Page::Overview => self.view_overview(&palette),
            Page::Cooling => self.view_cooling(&palette),
            Page::Monitor => self.view_monitor(&palette),
            Page::Keyboard => self.view_keyboard(&palette),
            Page::Profiles => self.view_profiles(&palette),
            Page::Cpu => self.view_generic_page(
                "CPU & Power Policy",
                "Configure governor, Energy Performance Preference (EPP), and frequency limits.",
                &palette,
            ),
            Page::Lightbar => self.view_generic_page(
                "Lightbar Control",
                "Front lightbar animations: Wave, Breath, Comet, Static, or Silent.",
                &palette,
            ),
            Page::Devices => self.view_generic_page(
                "Power Devices",
                "Matrix of allowed power devices, ASPM states, and Wi-Fi power save.",
                &palette,
            ),
            Page::Notifications => self.view_generic_page(
                "Notification Events",
                "Hardware notifications: Power source change, thermal throttling, water cooler.",
                &palette,
            ),
            Page::About => self.view_about(&palette),
        };

        let main_layout = column![top_bar, scrollable(content).height(Length::Fill)].spacing(0);

        row![sidebar, main_layout].into()
    }

    fn view_overview<'a>(&'a self, _palette: &Palette) -> Element<'a, Message> {
        let profiles_row = row![
            button(text("Quiet").size(13))
                .padding([10, 20])
                .on_press(Message::SelectProfile("Quiet".into())),
            button(text("Balanced").size(13))
                .padding([10, 20])
                .on_press(Message::SelectProfile("Balanced".into())),
            button(text("Performance").size(13))
                .padding([10, 20])
                .on_press(Message::SelectProfile("Performance".into())),
        ]
        .spacing(12);

        column![
            text("System Overview").size(18),
            text("Active Operating Mode:").size(14),
            profiles_row,
            Space::with_height(Length::Fixed(16.0)),
            text("Hardware Status").size(16),
            text("• CPU Temperature: 42°C | Power: 24W").size(13),
            text("• GPU: Runtime PM Suspended (D3cold)").size(13),
            text("• Water Cooler: Standby / Ready").size(13),
        ]
        .spacing(10)
        .padding(20)
        .into()
    }

    fn view_cooling<'a>(&'a self, _palette: &Palette) -> Element<'a, Message> {
        let canvas = FanCurveCanvas::new(self.fan_points.clone());

        column![
            text("Interactive Fan Curve Editor").size(18),
            text("Drag control points to adjust temperature threshold and fan duty cycle.")
                .size(13),
            canvas.view().map(Message::FanCurveEvent),
            Space::with_height(Length::Fixed(10.0)),
            row![
                text("Points: ").size(12),
                text(format!(
                    "P1: {}°C/{}% | P2: {}°C/{}% | P3: {}°C/{}% | P4: {}°C/{}% | P5: {}°C/{}%",
                    self.fan_points[0].temp_c,
                    self.fan_points[0].speed_pct,
                    self.fan_points[1].temp_c,
                    self.fan_points[1].speed_pct,
                    self.fan_points[2].temp_c,
                    self.fan_points[2].speed_pct,
                    self.fan_points[3].temp_c,
                    self.fan_points[3].speed_pct,
                    self.fan_points[4].temp_c,
                    self.fan_points[4].speed_pct,
                ))
                .size(12)
            ]
            .spacing(8)
        ]
        .spacing(10)
        .padding(20)
        .into()
    }

    fn view_monitor<'a>(&'a self, _palette: &Palette) -> Element<'a, Message> {
        let chart = TelemetryChartCanvas::new(self.telemetry_history.clone(), 60);

        column![
            text("Live Hardware Telemetry").size(18),
            text("Rolling 60-second horizon (Orange: CPU Temp, Green: GPU Temp)").size(13),
            chart.view(),
            Space::with_height(Length::Fixed(10.0)),
            text("Current Readings:").size(14),
            text("• CPU: 42°C | 24W").size(13),
            text("• GPU: 38°C | 0W (Suspended)").size(13),
            text("• Fan: 1800 RPM").size(13),
        ]
        .spacing(10)
        .padding(20)
        .into()
    }

    fn view_keyboard<'a>(&'a self, _palette: &Palette) -> Element<'a, Message> {
        let canvas = KeyboardCanvas::new(
            self.left_color,
            self.middle_color,
            self.right_color,
            self.keyboard_brightness,
        );

        column![
            text("Keyboard Backlight (3 Zones)").size(18),
            text("Visual zone layout with real-time color rendering:").size(13),
            canvas.view(),
            Space::with_height(Length::Fixed(10.0)),
            text(format!("Brightness: {}%", self.keyboard_brightness)).size(14),
            row![
                button(text("Blue Theme").size(12)).padding([6, 12]),
                button(text("White Theme").size(12)).padding([6, 12]),
                button(text("Red Theme").size(12)).padding([6, 12]),
            ]
            .spacing(10)
        ]
        .spacing(10)
        .padding(20)
        .into()
    }

    fn view_profiles<'a>(&'a self, _palette: &Palette) -> Element<'a, Message> {
        column![
            text("Profile Management").size(18),
            text("Configured Profiles:").size(14),
            text("• Quiet (Battery optimized, EPP power, 25W TDP)").size(13),
            text("• Balanced (Standard desktop performance, EPP balance_performance, 45W TDP)")
                .size(13),
            text("• Performance (Maximum performance, EPP performance, 75W TDP)").size(13),
        ]
        .spacing(12)
        .padding(20)
        .into()
    }

    fn view_about<'a>(&'a self, _palette: &Palette) -> Element<'a, Message> {
        column![
            text("About Frixit Control Center").size(18),
            text("Version: 0.3.0 (Rust Edition)").size(14),
            text("License: GPL-3.0-or-later").size(13),
            text("Toolkit: Iced v0.13 + wgpu (Qt/C++ free)").size(13),
            text("Backend: uccd (Tokio actors + /dev/tuxedo_io safe ioctl)").size(13),
        ]
        .spacing(10)
        .padding(20)
        .into()
    }

    fn view_generic_page<'a>(
        &'a self,
        title: &'static str,
        desc: &'static str,
        _palette: &Palette,
    ) -> Element<'a, Message> {
        column![
            text(title).size(18),
            text(desc).size(13),
            Space::with_height(Length::Fixed(16.0)),
            container(text("Configuration controls enabled in V1 prototype.").size(12)).padding(14),
        ]
        .spacing(10)
        .padding(20)
        .into()
    }
}
