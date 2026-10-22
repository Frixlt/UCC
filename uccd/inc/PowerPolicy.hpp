// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include "StateUtils.hpp"
#include <map>
#include <algorithm>
#include <cstdint>
#include <mutex>
#include <nlohmann/json.hpp>

// Runtime overrides are not persisted. Requests belong to the D-Bus connection,
// so a crashed application cannot leave a forced mode behind.
class PowerPolicy
{
public:
  struct Decision {
    ProfileState source;
    ProfileState effective;
    ProfileState profileState;
    nlohmann::json status;
  };

  bool setManualMode(const std::string &mode) {
    if (mode != "auto" && !validMode(mode)) return false;
    std::lock_guard lock(m_mutex);
    if (m_manual != mode) { m_manual = mode; ++m_revision; }
    return true;
  }

  bool registerRequest(const std::string &owner, const std::string &name,
                       const std::string &reason, const std::string &mode) {
    if (owner.empty() || name.empty() || name.size() > 128 || reason.size() > 512 ||
        (!mode.empty() && !validMode(mode))) return false;
    std::lock_guard lock(m_mutex);
    const auto key = std::make_pair(owner,name);
    if (!m_requests.contains(key)) {
      if (m_requests.size() >= 256) return false;
      size_t count = 0;
      for (const auto &[id, request] : m_requests) { (void)request; count += id.first == owner; }
      if (count >= 32) return false;
    }
    m_requests[key] = {reason,mode};
    return true;
  }

  bool unregisterRequest(const std::string &owner, const std::string &name) {
    std::lock_guard lock(m_mutex);
    if (!m_requests.erase({owner,name})) return false;
    return true;
  }

  void removeOwner(const std::string &owner) {
    std::lock_guard lock(m_mutex);
    std::erase_if(m_requests, [&owner](const auto &item) { return item.first.first == owner; });
  }

  Decision evaluate(ProfileState source, bool waterCoolerConnected) const {
    std::lock_guard lock(m_mutex);
    ProfileState effective = source;
    bool overridden = m_manual != "auto";
    int minimum = 0;
    auto requests = nlohmann::json::array();
    for (const auto &[id, request] : m_requests) {
      requests.push_back({{"owner",id.first},{"name",id.second},
                          {"reason",request.reason},{"force_mode",request.mode}});
      if (!request.mode.empty()) minimum = std::max(minimum,rank(modeState(request.mode)));
    }
    if (overridden) effective = modeState(m_manual);
    else if (minimum > rank(effective)) {
      overridden = true;
      if (minimum > rank(effective)) effective = minimum == 3 ? ProfileState::AC :
                                                 minimum == 2 ? ProfileState::TYPE_C : ProfileState::BAT;
    }
    const auto selected = !overridden && waterCoolerConnected ? ProfileState::WC : effective;
    nlohmann::json status = {{"real_power_source",modeName(source)},
      {"current_mode",modeName(effective)},{"manual_mode",m_manual},
      {"profile_state",profileStateToString(selected)},
      {"water_cooler_connected",waterCoolerConnected},{"registered_apps",requests},
      {"revision",m_revision}};
    return {source,effective,selected,std::move(status)};
  }

private:
  struct Request { std::string reason, mode; };
  static bool validMode(const std::string &mode) { return mode == "ac" || mode == "type_c" || mode == "bat"; }
  static ProfileState modeState(const std::string &mode) {
    return mode == "ac" ? ProfileState::AC : mode == "type_c" ? ProfileState::TYPE_C : ProfileState::BAT;
  }
  static const char *modeName(ProfileState state) {
    return state == ProfileState::AC ? "ac" : state == ProfileState::TYPE_C ? "type_c" : "bat";
  }
  static int rank(ProfileState state) { return state == ProfileState::AC ? 3 : state == ProfileState::TYPE_C ? 2 : 1; }
  mutable std::mutex m_mutex;
  std::string m_manual = "auto";
  uint64_t m_revision = 0;
  std::map<std::pair<std::string,std::string>,Request> m_requests;
};
