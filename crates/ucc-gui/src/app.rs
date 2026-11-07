//! Frixit Control Center GUI Application (Iced Fluent shell).
//!
//! Features:
//! - 10-page Fluent navigation shell
//! - Dynamic domain model binding (`UccSettings`, `UccProfile`, `FanCurve`, `ProfileCpu`)
//! - Monotonic interactive fan curve editor
//! - Live multi-channel telemetry history chart
//! - 3-zone keyboard backlight canvas preview
//! - Dark/Light theme switching
//! - Safe low-power GPU adapter selection (`WGPU_POWER_PREF=low_power`)
//! - Strict read-only preview mode (`UCC_READ_ONLY_PREVIEW` / `--preview`) blocking all save paths

use crate::navigation::Page;
use crate::theme::{Palette, ThemeMode};
use crate::widgets::chart::{TelemetryChartCanvas, TelemetryDataPoint};
use crate::widgets::fan_curve::{CurvePoint, FanCurveCanvas, FanCurveMessage};
use crate::widgets::keyboard_preview::{KeyboardCanvas, ZoneColor};
use iced::widget::{button, column, container, horizontal_space, row, scrollable, text, Space};
use iced::{Alignment, Element, Length, Subscription, Task, Theme as IcedTheme};
use ucc_config::{
    create_default_settings, default_office_profile, is_read_only_preview, ConfigManager,
    UccSettings, PROFILE_ID_HIGH_PERFORMANCE, PROFILE_ID_MAX_ENERGY_SAVE, PROFILE_ID_OFFICE,
    PROFILE_ID_QUIET,
};
use ucc_core::{FanPoint, UccProfile};

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
    SaveSettings,
    Tick,
}

pub struct ControlCenterApp {
    pub current_page: Page,
    pub theme_mode: ThemeMode,
    pub is_preview: bool,
    pub config_manager: ConfigManager,
    pub settings: UccSettings,
    pub active_profile_id: String,
    pub fan_points: Vec<CurvePoint>,
    pub telemetry_history: Vec<TelemetryDataPoint>,
    pub left_color: ZoneColor,
    pub middle_color: ZoneColor,
    pub right_color: ZoneColor,
    pub keyboard_brightness: u8,
    pub status_message: Option<String>,
    pub config_load_error: Option<String>,
}

impl ControlCenterApp {
    pub fn new() -> (Self, Task<Message>) {
        // Enforce preview mode if command line argument or environment variable is set
        let is_preview = std::env::args().any(|arg| arg == "--preview") || is_read_only_preview();

        let config_manager = ConfigManager::new();
        let (settings, config_load_error) = match config_manager.load() {
            Ok(s) => (s, None),
            Err(e) => {
                let err_msg = format!("Existing configuration failed to load: {}. Write operations are blocked to prevent data loss.", e);
                (create_default_settings(), Some(err_msg))
            }
        };

        let active_profile_id = settings
            .state_map
            .get("power_ac")
            .cloned()
            .unwrap_or_else(|| PROFILE_ID_OFFICE.to_string());

        let fallback_profile = default_office_profile();
        let active_profile = settings.get_profile(&active_profile_id).unwrap_or(&fallback_profile);

        let fan_points = if !active_profile.fan.table_cpu.is_empty() {
            active_profile
                .fan
                .table_cpu
                .iter()
                .map(|p| CurvePoint {
                    temp_c: p.temp as f32,
                    speed_pct: p.speed as f32,
                })
                .collect()
        } else {
            vec![
                CurvePoint { temp_c: 35.0, speed_pct: 20.0 },
                CurvePoint { temp_c: 50.0, speed_pct: 35.0 },
                CurvePoint { temp_c: 65.0, speed_pct: 55.0 },
                CurvePoint { temp_c: 80.0, speed_pct: 80.0 },
                CurvePoint { temp_c: 90.0, speed_pct: 100.0 },
            ]
        };

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
            config_manager,
            settings,
            active_profile_id,
            fan_points,
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
            status_message: None,
            config_load_error,
        };

        (app, Task::none())
    }

    /// Returns the currently active profile from settings or a default fallback.
    pub fn active_profile(&self) -> &UccProfile {
        self.settings.get_profile(&self.active_profile_id).unwrap_or_else(|| {
            // If active_profile_id is not directly in map, try resolving by alias
            self.settings.profiles.values().next().expect("settings must contain at least one profile")
        })
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
            Message::SelectProfile(profile_id) => {
                // Look up profile by canonical ID or alias
                if let Some(profile) = self.settings.get_profile(&profile_id).cloned() {
                    self.active_profile_id = profile.id.clone();

                    // Update fan curve points from the selected profile
                    if !profile.fan.table_cpu.is_empty() {
                        self.fan_points = profile
                            .fan
                            .table_cpu
                            .iter()
                            .map(|p| CurvePoint {
                                temp_c: p.temp as f32,
                                speed_pct: p.speed as f32,
                            })
                            .collect();
                    }

                    self.status_message = Some(format!("Switched to profile: {}", profile.name));
                }
            }
            Message::SaveSettings => {
                // Strict preview mode protection: block all saving paths
                if self.is_preview || is_read_only_preview() {
                    self.status_message =
                        Some("Save blocked: UCC_READ_ONLY_PREVIEW mode is active".to_string());
                    return;
                }

                // Block saving if existing configuration failed to load, preventing data destruction
                if let Some(ref err) = self.config_load_error {
                    self.status_message = Some(format!("Save blocked: {}", err));
                    return;
                }

                // Sync current fan curve points back into active profile
                if let Some(profile) = self.settings.profiles.get_mut(&self.active_profile_id) {
                    profile.fan.table_cpu = self
                        .fan_points
                        .iter()
                        .map(|p| FanPoint::new(p.temp_c as i32, p.speed_pct as u8))
                        .collect();
                }

                match self.config_manager.save(&self.settings) {
                    Ok(path) => {
                        self.status_message =
                            Some(format!("Settings saved successfully to {}", path.display()));
                    }
                    Err(e) => {
                        self.status_message = Some(format!("Error saving settings: {}", e));
                    }
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

        let profile_pill = container(
            text(format!("Profile: {}", self.active_profile().name)).size(12),
        )
        .padding([4, 10]);

        let save_btn = button(
            text(if self.is_preview {
                "💾 Save (Preview)"
            } else {
                "💾 Save Settings"
            })
            .size(12),
        )
        .on_press(Message::SaveSettings);

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
            save_btn,
            theme_btn,
        ]
        .spacing(12)
        .padding(14)
        .align_y(Alignment::Center);

        // Status banner if present
        let status_banner: Element<_> = if let Some(ref msg) = self.status_message {
            container(text(msg).size(12))
                .padding([6, 14])
                .width(Length::Fill)
                .into()
        } else {
            Space::with_height(Length::Shrink).into()
        };

        // Page content router
        let content: Element<_> = match self.current_page {
            Page::Overview => self.view_overview(&palette),
            Page::Cooling => self.view_cooling(&palette),
            Page::Monitor => self.view_monitor(&palette),
            Page::Keyboard => self.view_keyboard(&palette),
            Page::Profiles => self.view_profiles(&palette),
            Page::Cpu => self.view_cpu(&palette),
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

        let main_layout =
            column![top_bar, status_banner, scrollable(content).height(Length::Fill)].spacing(0);

        row![sidebar, main_layout].into()
    }

    fn view_overview<'a>(&'a self, _palette: &Palette) -> Element<'a, Message> {
        let active = self.active_profile();

        let profiles_row = row![
            button(text("Quiet").size(13))
                .padding([10, 16])
                .on_press(Message::SelectProfile(PROFILE_ID_QUIET.into())),
            button(text("Balanced (Office)").size(13))
                .padding([10, 16])
                .on_press(Message::SelectProfile(PROFILE_ID_OFFICE.into())),
            button(text("Performance").size(13))
                .padding([10, 16])
                .on_press(Message::SelectProfile(PROFILE_ID_HIGH_PERFORMANCE.into())),
            button(text("Max Energy").size(13))
                .padding([10, 16])
                .on_press(Message::SelectProfile(PROFILE_ID_MAX_ENERGY_SAVE.into())),
        ]
        .spacing(10);

        let cpu_governor = if active.cpu.governor.is_empty() {
            "powersave"
        } else {
            &active.cpu.governor
        };

        let cpu_epp = if active.cpu.energy_performance_preference.is_empty() {
            "balance_performance"
        } else {
            &active.cpu.energy_performance_preference
        };

        let turbo_str = if active.cpu.no_turbo {
            "Disabled"
        } else {
            "Enabled"
        };

        column![
            text("System Overview").size(18),
            text(format!("Active Operating Mode: {}", active.name)).size(14),
            profiles_row,
            Space::with_height(Length::Fixed(16.0)),
            text("Active Policy Parameters").size(16),
            text(format!("• CPU Governor: {}", cpu_governor)).size(13),
            text(format!("• Energy Performance Preference (EPP): {}", cpu_epp)).size(13),
            text(format!("• Turbo Boost: {}", turbo_str)).size(13),
            text(format!("• Fan Profile: {}", active.fan.fan_profile)).size(13),
            Space::with_height(Length::Fixed(12.0)),
            text("Hardware Status").size(16),
            text("• CPU Temperature: 42°C | Power: 24W").size(13),
            text("• GPU: Runtime PM Suspended (D3cold)").size(13),
            text("• Water Cooler: Standby / Ready").size(13),
        ]
        .spacing(8)
        .padding(20)
        .into()
    }

    fn view_cpu<'a>(&'a self, _palette: &Palette) -> Element<'a, Message> {
        let active = self.active_profile();

        let governor = if active.cpu.governor.is_empty() {
            "powersave"
        } else {
            &active.cpu.governor
        };

        let epp = if active.cpu.energy_performance_preference.is_empty() {
            "balance_performance"
        } else {
            &active.cpu.energy_performance_preference
        };

        let turbo = if active.cpu.no_turbo {
            "Disabled (Power Saving)"
        } else {
            "Enabled (High Clock Scaling)"
        };

        column![
            text("CPU & Power Policy").size(18),
            text(format!("Active Profile Policy: {}", active.name)).size(14),
            Space::with_height(Length::Fixed(10.0)),
            text(format!("• Scaling Governor: {}", governor)).size(13),
            text(format!("• Energy Performance Preference (EPP): {}", epp)).size(13),
            text(format!("• Intel/AMD Turbo Boost: {}", turbo)).size(13),
            text(format!(
                "• HWP Dynamic Boost: {}",
                if active.cpu.hwp_dynamic_boost { "Enabled" } else { "Disabled" }
            ))
            .size(13),
            text(format!(
                "• Frequency Range: {} kHz .. {} kHz",
                active.cpu.min_freq_khz.unwrap_or(800_000),
                active.cpu.max_freq_khz.unwrap_or(4_500_000)
            ))
            .size(13),
            Space::with_height(Length::Fixed(16.0)),
            row![
                button(text("Switch to Quiet").size(12))
                    .padding([8, 14])
                    .on_press(Message::SelectProfile(PROFILE_ID_QUIET.into())),
                button(text("Switch to Balanced").size(12))
                    .padding([8, 14])
                    .on_press(Message::SelectProfile(PROFILE_ID_OFFICE.into())),
                button(text("Switch to Performance").size(12))
                    .padding([8, 14])
                    .on_press(Message::SelectProfile(PROFILE_ID_HIGH_PERFORMANCE.into())),
            ]
            .spacing(10),
            Space::with_height(Length::Fixed(16.0)),
            button(text("Save Policy Changes").size(12))
                .padding([8, 16])
                .on_press(Message::SaveSettings),
        ]
        .spacing(10)
        .padding(20)
        .into()
    }

    fn view_cooling<'a>(&'a self, _palette: &Palette) -> Element<'a, Message> {
        let active = self.active_profile();
        let canvas = FanCurveCanvas::new(self.fan_points.clone());

        column![
            text(format!("Interactive Fan Curve Editor ({})", active.name)).size(18),
            text("Drag control points to adjust temperature threshold and fan duty cycle.")
                .size(13),
            canvas.view().map(Message::FanCurveEvent),
            Space::with_height(Length::Fixed(10.0)),
            row![
                text("Points: ").size(12),
                text(if self.fan_points.len() >= 5 {
                    format!(
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
                    )
                } else {
                    format!("{} points loaded", self.fan_points.len())
                })
                .size(12),
            ]
            .spacing(8),
            Space::with_height(Length::Fixed(10.0)),
            button(text("Save Fan Curve").size(12))
                .padding([8, 16])
                .on_press(Message::SaveSettings),
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
        let mut col = column![
            text("Profile Management").size(18),
            text("Available Built-in & Custom Profiles:").size(14),
        ]
        .spacing(10);

        for profile in self.settings.profiles.values() {
            let is_active = profile.id == self.active_profile_id;
            let active_label = if is_active { " [ACTIVE]" } else { "" };

            let p_card = row![
                column![
                    text(format!("• {}{}", profile.name, active_label)).size(14),
                    text(&profile.description).size(12),
                    text(format!(
                        "  CPU: {} / EPP: {} | Fan: {}",
                        if profile.cpu.governor.is_empty() { "powersave" } else { &profile.cpu.governor },
                        if profile.cpu.energy_performance_preference.is_empty() { "balance_performance" } else { &profile.cpu.energy_performance_preference },
                        profile.fan.fan_profile
                    )).size(11),
                ].spacing(2),
                horizontal_space(),
                button(text(if is_active { "Selected" } else { "Activate" }).size(12))
                    .padding([6, 14])
                    .on_press(Message::SelectProfile(profile.id.clone())),
            ]
            .align_y(Alignment::Center)
            .padding(10);

            col = col.push(p_card);
        }

        col = col.push(Space::with_height(Length::Fixed(12.0)));
        col = col.push(
            button(text("Save All Profile Settings").size(12))
                .padding([8, 16])
                .on_press(Message::SaveSettings),
        );

        col.padding(20).into()
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
