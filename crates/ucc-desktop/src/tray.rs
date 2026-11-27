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
enum ActionSender {
    Unbounded(mpsc::UnboundedSender<TrayAction>),
    Bounded(mpsc::Sender<TrayAction>),
}

impl ActionSender {
    fn send_action(&self, action: TrayAction) {
        match self {
            Self::Unbounded(tx) => {
                let _ = tx.send(action);
            }
            Self::Bounded(tx) => {
                if let Err(err) = tx.try_send(action) {
                    tracing::warn!("Dropping tray action {:?}: {err}", action);
                }
            }
        }
    }
}

#[derive(Clone)]
pub struct UccTray {
    profiles: Vec<String>,
    active_profile: Arc<AtomicUsize>,
    fan_auto: Arc<AtomicBool>,
    water_cooler_connected: Arc<AtomicBool>,
    daemon_connected: Arc<AtomicBool>,
    action_tx: ActionSender,
}

impl UccTray {
    pub const TRAY_ID: &'static str = "frixit-control-center";

    pub fn new(
        profiles: Vec<String>,
        active_profile_idx: usize,
        action_tx: mpsc::UnboundedSender<TrayAction>,
    ) -> Self {
        Self::with_sender(
            profiles,
            active_profile_idx,
            ActionSender::Unbounded(action_tx),
        )
    }

    pub fn new_bounded(
        profiles: Vec<String>,
        active_profile_idx: usize,
        action_tx: mpsc::Sender<TrayAction>,
    ) -> Self {
        Self::with_sender(
            profiles,
            active_profile_idx,
            ActionSender::Bounded(action_tx),
        )
    }

    fn with_sender(
        profiles: Vec<String>,
        active_profile_idx: usize,
        action_tx: ActionSender,
    ) -> Self {
        let valid_idx = if !profiles.is_empty() && active_profile_idx < profiles.len() {
            active_profile_idx
        } else {
            0
        };
        Self {
            profiles,
            active_profile: Arc::new(AtomicUsize::new(valid_idx)),
            fan_auto: Arc::new(AtomicBool::new(true)),
            water_cooler_connected: Arc::new(AtomicBool::new(false)),
            daemon_connected: Arc::new(AtomicBool::new(true)),
            action_tx,
        }
    }

    pub fn set_daemon_connected(&self, connected: bool) {
        self.daemon_connected.store(connected, Ordering::SeqCst);
    }

    pub fn is_daemon_connected(&self) -> bool {
        self.daemon_connected.load(Ordering::SeqCst)
    }

    pub fn set_active_profile(&self, index: usize) {
        if index < self.profiles.len() {
            self.active_profile.store(index, Ordering::SeqCst);
        }
    }

    pub fn set_profiles(&mut self, profiles: Vec<String>, active_idx: usize) {
        self.profiles = profiles;
        let valid_idx = if !self.profiles.is_empty() && active_idx < self.profiles.len() {
            active_idx
        } else {
            0
        };
        self.active_profile.store(valid_idx, Ordering::SeqCst);
    }

    pub fn profiles(&self) -> &[String] {
        &self.profiles
    }

    pub fn set_fan_auto(&self, fan_auto: bool) {
        self.fan_auto.store(fan_auto, Ordering::SeqCst);
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
        self.action_tx.send_action(TrayAction::OpenGui);
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
        let daemon_connected = self.daemon_connected.load(Ordering::SeqCst);

        let mut items = vec![MenuItem::Standard(StandardItem {
            label: "Open Control Center".into(),
            activate: Box::new(move |_| {
                open_tx.send_action(TrayAction::OpenGui);
            }),
            ..Default::default()
        })];

        if !daemon_connected {
            items.push(MenuItem::Standard(StandardItem {
                label: "Service unavailable (reconnecting...)".into(),
                enabled: false,
                ..Default::default()
            }));
        }

        items.extend([
            MenuItem::Separator,
            MenuItem::RadioGroup(RadioGroup {
                selected: current_profile,
                select: Box::new(move |this: &mut Self, idx: usize| {
                    if this.daemon_connected.load(Ordering::SeqCst) && idx < this.profiles.len() {
                        profile_tx.send_action(TrayAction::SelectProfile(idx));
                    }
                }),
                options: profiles
                    .into_iter()
                    .map(|name| RadioItem {
                        label: name,
                        enabled: daemon_connected,
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
                enabled: daemon_connected,
                activate: Box::new(move |this: &mut Self| {
                    if this.daemon_connected.load(Ordering::SeqCst) {
                        fan_tx.send_action(TrayAction::ToggleFanAuto);
                    }
                }),
                ..Default::default()
            }),
            MenuItem::Standard(StandardItem {
                label: if wc_connected {
                    "Water Cooler: Connected (click to disconnect)".into()
                } else {
                    "Water Cooler: Disconnected (click to scan)".into()
                },
                enabled: daemon_connected,
                activate: Box::new(move |this: &mut Self| {
                    if this.daemon_connected.load(Ordering::SeqCst) {
                        wc_tx.send_action(TrayAction::ToggleWaterCooler);
                    }
                }),
                ..Default::default()
            }),
            MenuItem::Separator,
            MenuItem::Standard(StandardItem {
                label: "Quit".into(),
                activate: Box::new(move |_| {
                    quit_tx.send_action(TrayAction::Quit);
                }),
                ..Default::default()
            }),
        ]);

        items
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

/// Spawns the StatusNotifierItem tray using a bounded action channel.
pub async fn spawn_tray_bounded(
    profiles: Vec<String>,
    active_profile_idx: usize,
    action_tx: mpsc::Sender<TrayAction>,
) -> Result<ksni::Handle<UccTray>, ksni::Error> {
    let tray = UccTray::new_bounded(profiles, active_profile_idx, action_tx);
    tray.spawn().await
}

pub async fn update_daemon_connected(handle: &ksni::Handle<UccTray>, connected: bool) {
    let _ = handle
        .update(move |tray| {
            tray.set_daemon_connected(connected);
        })
        .await;
}

pub async fn update_active_profile(handle: &ksni::Handle<UccTray>, index: usize) {
    let _ = handle
        .update(move |tray| {
            tray.set_active_profile(index);
        })
        .await;
}

pub async fn update_profiles(
    handle: &ksni::Handle<UccTray>,
    profiles: Vec<String>,
    active_idx: usize,
) {
    let _ = handle
        .update(move |tray| {
            tray.set_profiles(profiles, active_idx);
        })
        .await;
}

pub async fn update_fan_auto(handle: &ksni::Handle<UccTray>, fan_auto: bool) {
    let _ = handle
        .update(move |tray| {
            tray.set_fan_auto(fan_auto);
        })
        .await;
}

pub async fn update_water_cooler_connected(handle: &ksni::Handle<UccTray>, connected: bool) {
    let _ = handle
        .update(move |tray| {
            tray.set_water_cooler_connected(connected);
        })
        .await;
}
