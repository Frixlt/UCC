//! Navigation pages for Frixit Control Center.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Page {
    #[default]
    Overview,
    Monitor,
    Profiles,
    Cpu,
    Cooling,
    Keyboard,
    Lightbar,
    Devices,
    Notifications,
    About,
}

impl Page {
    pub const ALL: [Page; 10] = [
        Page::Overview,
        Page::Monitor,
        Page::Profiles,
        Page::Cpu,
        Page::Cooling,
        Page::Keyboard,
        Page::Lightbar,
        Page::Devices,
        Page::Notifications,
        Page::About,
    ];

    pub fn title(&self) -> &'static str {
        match self {
            Page::Overview => "Overview",
            Page::Monitor => "Hardware Monitor",
            Page::Profiles => "Performance Profiles",
            Page::Cpu => "CPU & Power Policy",
            Page::Cooling => "Cooling & Fans",
            Page::Keyboard => "Keyboard Backlight",
            Page::Lightbar => "Lightbar Control",
            Page::Devices => "Power Devices",
            Page::Notifications => "Notifications",
            Page::About => "About & Diagnostics",
        }
    }

    pub fn icon(&self) -> &'static str {
        match self {
            Page::Overview => "📊",
            Page::Monitor => "📈",
            Page::Profiles => "⚡",
            Page::Cpu => "🔲",
            Page::Cooling => "❄️",
            Page::Keyboard => "⌨️",
            Page::Lightbar => "💡",
            Page::Devices => "🔌",
            Page::Notifications => "🔔",
            Page::About => "ℹ️",
        }
    }
}
