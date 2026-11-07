//! Fluent theme definitions for Frixit Control Center.

use iced::Color;

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    Auto,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy)]
pub struct Palette {
    pub background: Color,
    pub surface: Color,
    pub surface_hover: Color,
    pub primary: Color,
    pub primary_hover: Color,
    pub text_main: Color,
    pub text_muted: Color,
    pub border: Color,
    pub success: Color,
    pub warning: Color,
    pub danger: Color,
}

impl Palette {
    pub const DARK: Palette = Palette {
        background: Color::from_rgb(0.12, 0.12, 0.12), // #1f1f1f
        surface: Color::from_rgb(0.18, 0.18, 0.18),    // #2d2d2d
        surface_hover: Color::from_rgb(0.24, 0.24, 0.24),
        primary: Color::from_rgb(0.0, 0.47, 0.83), // #0078d4 Fluent Blue
        primary_hover: Color::from_rgb(0.1, 0.55, 0.9),
        text_main: Color::from_rgb(0.95, 0.95, 0.95),
        text_muted: Color::from_rgb(0.65, 0.65, 0.65),
        border: Color::from_rgb(0.28, 0.28, 0.28),
        success: Color::from_rgb(0.2, 0.8, 0.4),
        warning: Color::from_rgb(0.95, 0.75, 0.2),
        danger: Color::from_rgb(0.9, 0.25, 0.25),
    };

    pub const LIGHT: Palette = Palette {
        background: Color::from_rgb(0.95, 0.95, 0.95), // #f3f3f3
        surface: Color::from_rgb(1.0, 1.0, 1.0),       // #ffffff
        surface_hover: Color::from_rgb(0.92, 0.92, 0.92),
        primary: Color::from_rgb(0.0, 0.47, 0.83),
        primary_hover: Color::from_rgb(0.1, 0.55, 0.9),
        text_main: Color::from_rgb(0.1, 0.1, 0.1),
        text_muted: Color::from_rgb(0.45, 0.45, 0.45),
        border: Color::from_rgb(0.85, 0.85, 0.85),
        success: Color::from_rgb(0.15, 0.65, 0.3),
        warning: Color::from_rgb(0.85, 0.6, 0.1),
        danger: Color::from_rgb(0.85, 0.2, 0.2),
    };

    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::DARK,
            ThemeMode::Light => Self::LIGHT,
            ThemeMode::Auto => Self::DARK, // Default to dark in prototype
        }
    }
}
