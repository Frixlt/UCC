// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include "TccSettings.hpp"
#include <filesystem>
#include <fstream>
#include <string>

// Inspect all supplies: firmware may report both USB-C and Mains online.
// USB-C takes precedence, matching the user's existing ControlCenter.
inline ProfileState determineState(
  const std::filesystem::path &root = "/sys/class/power_supply" ) noexcept
{
  try {
    if (!std::filesystem::exists(root)) return ProfileState::AC;
    bool mains = false, usb = false, knownSupply = false;
    for (const auto &entry : std::filesystem::directory_iterator(root)) {
      if (!entry.is_directory()) continue;
      std::string type, online;
      std::ifstream(entry.path()/"type") >> type;
      std::ifstream(entry.path()/"online") >> online;
      const auto name = entry.path().filename().string();
      const bool isUsb = type.starts_with("USB") || name.starts_with("ucsi") ||
                         name.starts_with("USB");
      const bool isMains = type == "Mains";
      knownSupply |= isUsb || isMains || type == "Battery";
      if (online != "1") continue;
      usb |= isUsb;
      mains |= isMains;
    }
    if (usb) return ProfileState::TYPE_C;
    if (mains) return ProfileState::AC;
    return knownSupply ? ProfileState::BAT : ProfileState::AC;
  } catch (...) {
    return ProfileState::AC;
  }
}

inline std::string profileStateToString( ProfileState state ) noexcept
{
  switch (state) {
    case ProfileState::AC: return "power_ac";
    case ProfileState::BAT: return "power_bat";
    case ProfileState::WC: return "power_wc";
    case ProfileState::TYPE_C: return "power_type_c";
    default: return "power_ac";
  }
}
