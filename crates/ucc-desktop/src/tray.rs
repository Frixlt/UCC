//! StatusNotifierItem (SNI) system tray service for Frixit Control Center.
//!
//! Provides desktop panel integration for KDE Plasma, Hyprland (waybar), GNOME (via AppIndicator),
//! and X11 without requiring Qt or C++ runtime dependencies.

use ksni::menu::{RadioGroup, RadioItem, StandardItem};
use ksni::{MenuItem, Tray, TrayMethods};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    OpenGui,
    SelectProfile(usize),
    ToggleFanAuto,
    ToggleWaterCooler,
    Quit,
}

#[derive(Clone)]
pub struct UccTray {
    profiles: Vec<String>,
    active_profile: Arc<AtomicUsize>,
    fan_auto: Arc<AtomicBool>,
    water_cooler_connected: Arc<AtomicBool>,
    action_tx: mpsc::UnboundedSender<TrayAction>,
}

impl UccTray {
    pub const TRAY_ID: &'static str = "frixit-control-center";

    pub fn new(
        profiles: Vec<String>,
        active_profile_idx: usize,
        action_tx: mpsc::UnboundedSender<TrayAction>,
    ) -> Self {
        Self {
            profiles,
            active_profile: Arc::new(AtomicUsize::new(active_profile_idx)),
            fan_auto: Arc::new(AtomicBool::new(true)),
            water_cooler_connected: Arc::new(AtomicBool::new(false)),
            action_tx,
        }
    }

    pub fn set_active_profile(&self, index: usize) {
        if index < self.profiles.len() {
            self.active_profile.store(index, Ordering::SeqCst);
        }
    }

    pub fn set_water_cooler_connected(&self, connected: bool) {
        self.water_cooler_connected
            .store(connected, Ordering::SeqCst);
    }
}

impl Tray for UccTray {
    fn id(&self) -> String {
        Self::TRAY_ID.to_string()
    }

    fn title(&self) -> String {
        "Frixit Control Center".to_string()
    }

    fn icon_name(&self) -> String {
        "preferences-system-performance".to_string()
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        let _ = self.action_tx.send(TrayAction::OpenGui);
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let current_profile = self.active_profile.load(Ordering::SeqCst);
        let profiles = self.profiles.clone();

        let profile_tx = self.action_tx.clone();
        let open_tx = self.action_tx.clone();
        let fan_tx = self.action_tx.clone();
        let wc_tx = self.action_tx.clone();
        let quit_tx = self.action_tx.clone();

        let wc_connected = self.water_cooler_connected.load(Ordering::SeqCst);
        let fan_auto = self.fan_auto.load(Ordering::SeqCst);

        vec![
            MenuItem::Standard(StandardItem {
                label: "Open Control Center".into(),
                activate: Box::new(move |_| {
                    let _ = open_tx.send(TrayAction::OpenGui);
                }),
                ..Default::default()
            }),
            MenuItem::Separator,
            MenuItem::RadioGroup(RadioGroup {
                selected: current_profile,
                select: Box::new(move |this: &mut Self, idx: usize| {
                    this.active_profile.store(idx, Ordering::SeqCst);
                    let _ = profile_tx.send(TrayAction::SelectProfile(idx));
                }),
                options: profiles
                    .into_iter()
                    .map(|name| RadioItem {
                        label: name,
                        ..Default::default()
                    })
                    .collect(),
            }),
            MenuItem::Separator,
            MenuItem::Standard(StandardItem {
                label: if fan_auto {
                    "Fan: Auto (click for Max)".into()
                } else {
                    "Fan: Max (click for Auto)".into()
                },
                activate: Box::new(move |this: &mut Self| {
                    let new_val = !this.fan_auto.load(Ordering::SeqCst);
                    this.fan_auto.store(new_val, Ordering::SeqCst);
                    let _ = fan_tx.send(TrayAction::ToggleFanAuto);
                }),
                ..Default::default()
            }),
            MenuItem::Standard(StandardItem {
                label: if wc_connected {
                    "Water Cooler: Connected (click to disconnect)".into()
                } else {
                    "Water Cooler: Disconnected (click to scan)".into()
                },
                activate: Box::new(move |_| {
                    let _ = wc_tx.send(TrayAction::ToggleWaterCooler);
                }),
                ..Default::default()
            }),
            MenuItem::Separator,
            MenuItem::Standard(StandardItem {
                label: "Quit".into(),
                activate: Box::new(move |_| {
                    let _ = quit_tx.send(TrayAction::Quit);
                }),
                ..Default::default()
            }),
        ]
    }
}

/// Spawns the StatusNotifierItem tray in the background
pub async fn spawn_tray(
    profiles: Vec<String>,
    active_profile_idx: usize,
    action_tx: mpsc::UnboundedSender<TrayAction>,
) -> Result<ksni::Handle<UccTray>, ksni::Error> {
    let tray = UccTray::new(profiles, active_profile_idx, action_tx);
    tray.spawn().await
}
