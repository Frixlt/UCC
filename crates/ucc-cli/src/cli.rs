//! CLI definition with clap, subcommands, aliases, completions, and manpages.

use std::io::Write;
use clap::{Args, CommandFactory, Parser, Subcommand};
use clap_complete::Shell;

#[derive(Parser, Debug)]
#[command(
    name = "ucc-cli",
    about = "Command-line interface for Frixit / Uniwill Control Center",
    version,
    arg_required_else_help = true
)]
pub struct Cli {
    /// Output structured JSON instead of human-readable text
    #[arg(short = 'j', long = "json", global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Commands {
    /// Print a full system dashboard (CPU, GPU, fans, hardware, water cooler, charging)
    Status,

    /// Continuously print telemetry summary (temperature, frequency, power, fans)
    #[command(visible_alias = "mon")]
    Monitor(MonitorArgs),

    /// Performance and power profile management
    #[command(visible_alias = "prof")]
    Profile {
        #[command(subcommand)]
        action: ProfileAction,
    },

    /// Power-state to profile mapping (AC, Battery, Type-C, Water Cooler)
    #[command(visible_alias = "state-map")]
    Statemap {
        #[command(subcommand)]
        action: StatemapAction,
    },

    /// Fan profiles and curve management
    Fan {
        #[command(subcommand)]
        action: FanAction,
    },

    /// Keyboard backlight configuration and profiles
    #[command(visible_alias = "kb")]
    Keyboard {
        #[command(subcommand)]
        action: KeyboardAction,
    },

    /// Liquid cooling (LCT) water cooler status and control
    #[command(visible_aliases = ["watercooler", "wc"])]
    Cooler {
        #[command(subcommand)]
        action: CoolerAction,
    },

    /// Battery charging profile and threshold management
    #[command(visible_alias = "charge")]
    Charging {
        #[command(subcommand)]
        action: ChargingAction,
    },

    /// CPU frequency limits, governors, EPP, and core telemetry
    Cpu {
        #[command(subcommand)]
        action: Option<CpuAction>,
    },

    /// Discrete and integrated GPU sensors, utilization, and power limits
    Gpu,

    /// View ODM power limits (PL1, PL2, PL4)
    #[command(visible_alias = "odm")]
    PowerLimits,

    /// Query or set display brightness (0-100)
    #[command(visible_alias = "br")]
    Brightness {
        #[command(subcommand)]
        action: BrightnessAction,
    },

    /// Query or toggle software webcam switch
    Webcam {
        #[command(subcommand)]
        action: ToggleAction,
    },

    /// Query or toggle Fn-Lock status
    #[command(visible_alias = "fn-lock")]
    Fnlock {
        #[command(subcommand)]
        action: ToggleAction,
    },

    /// Lightbar RGB animation modes, colors, and triggers
    Lightbar {
        #[command(subcommand)]
        action: LightbarAction,
    },

    /// Connected power device status and scanning
    Devices {
        #[command(subcommand)]
        action: DevicesAction,
    },

    /// Power policy and manual power mode switching
    Power {
        #[command(subcommand)]
        action: PowerAction,
    },

    /// Generate shell completion scripts (bash, zsh, fish, powershell, elvish)
    Completions {
        /// Shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },

    /// Generate groff man page to stdout
    Man,
}

#[derive(Args, Debug, Clone)]
pub struct MonitorArgs {
    /// Number of iterations to sample (0 = infinite)
    #[arg(short = 'n', long = "count", default_value_t = 0)]
    pub count: i32,

    /// Polling interval in seconds
    #[arg(short = 'i', long = "interval", default_value_t = 2)]
    pub interval: u64,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ProfileAction {
    /// List all built-in and custom profiles
    #[command(visible_alias = "ls")]
    List,

    /// Get active profile or profile by ID as JSON
    #[command(visible_aliases = ["show", "active"])]
    Get {
        /// Profile ID (defaults to currently active profile if omitted)
        id: Option<String>,
    },

    /// Activate profile by ID
    #[command(visible_alias = "activate")]
    Set {
        /// Profile ID to activate
        id: String,
    },

    /// Show built-in default profiles
    #[command(visible_alias = "default")]
    Defaults,

    /// Show custom user profiles
    #[command(visible_alias = "custom")]
    Customs,

    /// Apply profile from JSON string
    Apply {
        /// Profile JSON string
        json: String,
    },

    /// Save custom profile from JSON string
    #[command(visible_alias = "create")]
    Save {
        /// Profile JSON string
        json: String,
    },

    /// Delete custom profile by ID
    #[command(visible_aliases = ["del", "rm"])]
    Delete {
        /// Profile ID to delete
        id: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum StatemapAction {
    /// Get current state mapping and feature settings
    #[command(visible_alias = "show")]
    Get,

    /// Bind profile to power state (e.g. power_ac, power_bat, power_wc, power_type_c)
    Set {
        /// Power state name
        state: String,
        /// Profile ID
        profile_id: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum FanAction {
    /// List available fan profiles
    #[command(visible_alias = "ls")]
    List,

    /// Show fan curve points for given profile ID
    #[command(visible_alias = "show")]
    Get {
        /// Fan profile ID
        id: String,
    },

    /// Activate fan profile by ID
    #[command(visible_alias = "activate")]
    Set {
        /// Fan profile ID
        id: String,
    },

    /// Apply fan curve tables from JSON
    Apply {
        /// Fan profiles JSON
        json: String,
    },

    /// Revert fan profiles to profile defaults
    Revert,
}

#[derive(Subcommand, Debug, Clone)]
pub enum KeyboardAction {
    /// Show keyboard backlight capabilities (zones, modes, colors)
    #[command(visible_alias = "caps")]
    Info,

    /// Show current keyboard backlight state
    #[command(visible_alias = "show")]
    Get,

    /// Apply keyboard backlight configuration JSON
    Set {
        /// Backlight JSON string
        json: String,
    },

    /// Set solid RGB color and optional brightness across all zones
    Color {
        /// Red channel (0-255)
        r: i32,
        /// Green channel (0-255)
        g: i32,
        /// Blue channel (0-255)
        b: i32,
        /// Optional zone brightness (0-255, default 128)
        brightness: Option<i32>,
    },

    /// List saved keyboard backlight profiles
    #[command(visible_aliases = ["profile-list", "ls"])]
    Profiles,

    /// Activate keyboard profile by ID
    #[command(visible_alias = "use")]
    Activate {
        /// Keyboard profile ID
        id: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum CoolerAction {
    /// Show water cooler status, pump level, and fan speed
    Status,

    /// Enable water cooler connection
    #[command(visible_alias = "on")]
    Enable,

    /// Disable water cooler connection
    #[command(visible_alias = "off")]
    Disable,

    /// Set water cooler fan speed percentage (0-100)
    Fan {
        /// Duty cycle percent (0-100)
        percent: i32,
    },

    /// Set water cooler pump level code (1=Low, 2=Med, 3=High)
    Pump {
        /// Voltage/speed code
        code: i32,
    },

    /// Set water cooler LED RGB and mode
    Led {
        /// Red (0-255)
        r: i32,
        /// Green (0-255)
        g: i32,
        /// Blue (0-255)
        b: i32,
        /// Mode (0=static, 1=breathing, etc.)
        mode: i32,
    },

    /// Turn off water cooler LED
    #[command(visible_alias = "led_off")]
    LedOff,
}

#[derive(Subcommand, Debug, Clone)]
pub enum ChargingAction {
    /// Show current charging profile, priorities, and thresholds
    Status,

    /// Set active charging profile descriptor
    #[command(visible_alias = "set_profile")]
    SetProfile {
        /// Profile descriptor (e.g. standard, balanced, full)
        profile: String,
    },

    /// Set charging priority descriptor
    #[command(visible_alias = "set_priority")]
    SetPriority {
        /// Priority descriptor
        priority: String,
    },

    /// Set battery charge start and end thresholds (percent)
    #[command(visible_alias = "set_thresholds")]
    SetThresholds {
        /// Start charging threshold %
        start: i32,
        /// End / stop charging threshold %
        end: i32,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum CpuAction {
    /// Show CPU core count, frequencies, governors, and EPP
    Status,

    /// Query or set CPU scaling governor
    Governor {
        /// Governor name to set (optional)
        name: Option<String>,
    },

    /// Query or set Energy Performance Preference (EPP)
    Epp {
        /// EPP preference to set (optional)
        preference: Option<String>,
    },

    /// Set CPU minimum and maximum frequency limits (MHz)
    Frequency {
        /// Min frequency in MHz
        min: i32,
        /// Max frequency in MHz
        max: i32,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum BrightnessAction {
    /// Read display brightness percentage (0-100)
    Get,

    /// Set display brightness percentage (0-100)
    Set {
        /// Brightness value (0-100)
        value: i32,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum ToggleAction {
    /// Read switch status
    Get,

    /// Set switch state (on/off, true/false, 1/0)
    Set {
        /// Desired state
        value: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum LightbarAction {
    /// Read lightbar configuration settings
    #[command(visible_alias = "status")]
    Get,

    /// Set lightbar settings from JSON string
    Set {
        /// Settings JSON string
        json: String,
    },

    /// Trigger test animation
    Test,

    /// Trigger notification animation
    Notify,
}

#[derive(Subcommand, Debug, Clone)]
pub enum DevicesAction {
    /// Read power device settings or status
    #[command(visible_alias = "status")]
    Get,

    /// Trigger scan for power devices
    Scan,

    /// Set power device settings from JSON
    Set {
        /// Device settings JSON
        json: String,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum PowerAction {
    /// Read power policy status
    Status,

    /// Set manual power mode (auto, ac, type_c, bat)
    Mode {
        /// Mode name
        mode: String,
    },
}

/// Helper to generate shell completion script.
pub fn generate_completions(shell: Shell, out: &mut dyn Write) {
    let mut cmd = Cli::command();
    clap_complete::generate(shell, &mut cmd, "ucc-cli", out);
}

/// Helper to generate groff manpage.
pub fn generate_manpage(out: &mut dyn Write) -> std::io::Result<()> {
    let cmd = Cli::command();
    let man = clap_mangen::Man::new(cmd);
    man.render(out)
}
