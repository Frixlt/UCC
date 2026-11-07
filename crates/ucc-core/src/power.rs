use crate::error::ValidationError;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

/// System profile power state, identifying the currently applicable configuration mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ProfileState {
    /// AC mains power ("power_ac")
    Ac,
    /// Battery power ("power_bat")
    Bat,
    /// Water cooler connected profile state ("power_wc")
    Wc,
    /// USB Type-C Power Delivery ("power_type_c")
    TypeC,
}

impl ProfileState {
    /// Canonical configuration wire string (e.g. "power_ac").
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ac => "power_ac",
            Self::Bat => "power_bat",
            Self::Wc => "power_wc",
            Self::TypeC => "power_type_c",
        }
    }

    /// Short mode identifier (e.g. "ac", "type_c", "bat").
    pub const fn mode_name(self) -> &'static str {
        match self {
            Self::Ac => "ac",
            Self::Bat => "bat",
            Self::Wc => "wc",
            Self::TypeC => "type_c",
        }
    }

    /// Rank for automatic elevation hierarchy: AC (3) > Type-C (2) > Bat (1).
    pub const fn rank(self) -> u8 {
        match self {
            Self::Ac => 3,
            Self::TypeC => 2,
            Self::Bat => 1,
            Self::Wc => 0,
        }
    }
}

impl fmt::Display for ProfileState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ProfileState {
    type Err = ValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "power_ac" | "ac" => Ok(Self::Ac),
            "power_bat" | "bat" => Ok(Self::Bat),
            "power_wc" | "wc" => Ok(Self::Wc),
            "power_type_c" | "type_c" => Ok(Self::TypeC),
            other => Err(ValidationError::InvalidProfileState(other.to_string())),
        }
    }
}

impl Serialize for ProfileState {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ProfileState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Power modes applicable as physical supply or operational modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum PowerMode {
    Ac,
    TypeC,
    Bat,
}

impl PowerMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Ac => "ac",
            Self::TypeC => "type_c",
            Self::Bat => "bat",
        }
    }

    pub const fn rank(self) -> u8 {
        match self {
            Self::Ac => 3,
            Self::TypeC => 2,
            Self::Bat => 1,
        }
    }

    pub const fn to_profile_state(self) -> ProfileState {
        match self {
            Self::Ac => ProfileState::Ac,
            Self::TypeC => ProfileState::TypeC,
            Self::Bat => ProfileState::Bat,
        }
    }
}

impl fmt::Display for PowerMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PowerMode {
    type Err = ValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ac" => Ok(Self::Ac),
            "type_c" => Ok(Self::TypeC),
            "bat" => Ok(Self::Bat),
            other => Err(ValidationError::InvalidPowerMode(other.to_string())),
        }
    }
}

impl Serialize for PowerMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for PowerMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Manual power mode override, allowing "auto" or a forced power mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ManualPowerMode {
    #[default]
    Auto,
    Ac,
    TypeC,
    Bat,
}

impl ManualPowerMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Ac => "ac",
            Self::TypeC => "type_c",
            Self::Bat => "bat",
        }
    }
}

impl fmt::Display for ManualPowerMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for ManualPowerMode {
    type Err = ValidationError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "auto" => Ok(Self::Auto),
            "ac" => Ok(Self::Ac),
            "type_c" => Ok(Self::TypeC),
            "bat" => Ok(Self::Bat),
            other => Err(ValidationError::InvalidPowerMode(other.to_string())),
        }
    }
}

impl Serialize for ManualPowerMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ManualPowerMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// Registered application power mode request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppPowerRequest {
    pub reason: String,
    pub mode: String,
}

/// Evaluation decision produced by `PowerPolicy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PolicyDecision {
    /// Physical supply detected (or source profile state)
    pub source: ProfileState,
    /// Effective operational power mode (AC, Type-C, or Bat)
    pub effective: ProfileState,
    /// Final selected profile state (may be WC if cooler connected and not overridden)
    pub profile_state: ProfileState,
    /// Status JSON payload matching D-Bus contract
    pub status: serde_json::Value,
}

/// Power policy manager maintaining manual overrides and ephemeral client requests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PowerPolicy {
    manual_mode: ManualPowerMode,
    revision: u64,
    requests: BTreeMap<(String, String), AppPowerRequest>,
}

impl Default for PowerPolicy {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerPolicy {
    pub fn new() -> Self {
        Self {
            manual_mode: ManualPowerMode::Auto,
            revision: 0,
            requests: BTreeMap::new(),
        }
    }

    pub fn manual_mode(&self) -> ManualPowerMode {
        self.manual_mode
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn requests(&self) -> &BTreeMap<(String, String), AppPowerRequest> {
        &self.requests
    }

    /// Sets the manual power mode override ("auto", "ac", "type_c", "bat").
    /// Rejects invalid modes (e.g. "power_wc" or unknown strings).
    pub fn set_manual_mode(&mut self, mode_str: &str) -> bool {
        let parsed = match mode_str.parse::<ManualPowerMode>() {
            Ok(m) => m,
            Err(_) => return false,
        };

        if self.manual_mode != parsed {
            self.manual_mode = parsed;
            self.revision = self.revision.wrapping_add(1);
        }
        true
    }

    /// Registers or updates an application power mode request.
    ///
    /// Validates owner non-empty, name length 1..=128, reason length <= 512,
    /// and mode is either empty or one of {"ac", "type_c", "bat"}.
    /// Enforces max 256 total requests and max 32 requests per owner.
    pub fn register_request(
        &mut self,
        owner: &str,
        name: &str,
        reason: &str,
        mode: &str,
    ) -> bool {
        if owner.is_empty() || name.is_empty() || name.len() > 128 || reason.len() > 512 {
            return false;
        }

        if !mode.is_empty() && mode != "ac" && mode != "type_c" && mode != "bat" {
            return false;
        }

        let key = (owner.to_string(), name.to_string());
        if !self.requests.contains_key(&key) {
            if self.requests.len() >= 256 {
                return false;
            }
            let owner_count = self.requests.keys().filter(|k| k.0 == owner).count();
            if owner_count >= 32 {
                return false;
            }
        }

        self.requests.insert(
            key,
            AppPowerRequest {
                reason: reason.to_string(),
                mode: mode.to_string(),
            },
        );
        true
    }

    /// Unregisters an existing application request.
    pub fn unregister_request(&mut self, owner: &str, name: &str) -> bool {
        self.requests.remove(&(owner.to_string(), name.to_string())).is_some()
    }

    /// Removes all requests belonging to the given owner (e.g. when D-Bus client disconnects).
    pub fn remove_owner(&mut self, owner: &str) {
        self.requests.retain(|k, _| k.0 != owner);
    }

    /// Evaluates current effective power mode and profile state given physical supply and cooler connection.
    pub fn evaluate(&self, source: ProfileState, water_cooler_connected: bool) -> PolicyDecision {
        let mut effective = match source {
            ProfileState::Wc => ProfileState::Ac,
            other => other,
        };

        let forced = self.manual_mode != ManualPowerMode::Auto;
        let mut minimum_rank = 0u8;
        let mut registered_apps = Vec::new();

        for ((owner, name), req) in &self.requests {
            registered_apps.push(serde_json::json!({
                "owner": owner,
                "name": name,
                "reason": req.reason,
                "force_mode": req.mode,
            }));

            if !req.mode.is_empty() {
                if let Ok(m) = req.mode.parse::<PowerMode>() {
                    minimum_rank = minimum_rank.max(m.rank());
                }
            }
        }

        let mut overridden = forced;
        if forced {
            effective = match self.manual_mode {
                ManualPowerMode::Ac => ProfileState::Ac,
                ManualPowerMode::TypeC => ProfileState::TypeC,
                ManualPowerMode::Bat => ProfileState::Bat,
                ManualPowerMode::Auto => unreachable!(),
            };
        } else if minimum_rank > effective.rank() {
            overridden = true;
            effective = match minimum_rank {
                3 => ProfileState::Ac,
                2 => ProfileState::TypeC,
                _ => ProfileState::Bat,
            };
        }

        let selected = if !overridden && water_cooler_connected {
            ProfileState::Wc
        } else {
            effective
        };

        let status = serde_json::json!({
            "real_power_source": source.mode_name(),
            "current_mode": effective.mode_name(),
            "manual_mode": self.manual_mode.as_str(),
            "profile_state": selected.as_str(),
            "water_cooler_connected": water_cooler_connected,
            "registered_apps": registered_apps,
            "revision": self.revision,
        });

        PolicyDecision {
            source,
            effective,
            profile_state: selected,
            status,
        }
    }
}
