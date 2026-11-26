// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once

#include <algorithm>
#include <cctype>
#include <filesystem>
#include <fstream>
#include <set>
#include <sstream>
#include <string>
#include <system_error>
#include <vector>

struct WakeupApplyResult {
  int enabled = 0;
  int disabled = 0;
  int failed = 0;
};

namespace wakeup_policy_detail {

inline std::string trimAscii(std::string value) {
  auto notSpace = [](unsigned char ch) { return !std::isspace(ch); };
  value.erase(value.begin(), std::find_if(value.begin(), value.end(), notSpace));
  value.erase(std::find_if(value.rbegin(), value.rend(), notSpace).base(), value.end());
  return value;
}

inline std::string readTrimmed(const std::filesystem::path &path) {
  std::ifstream file(path);
  if (!file)
    return {};
  std::string content((std::istreambuf_iterator<char>(file)), std::istreambuf_iterator<char>());
  return trimAscii(std::move(content));
}

inline bool writeValue(const std::filesystem::path &path, const std::string &value) {
  std::ofstream file(path);
  if (!file)
    return false;
  file << value;
  file.flush();
  return static_cast<bool>(file);
}

inline bool isAllowedWakeupDevice(const std::filesystem::path &deviceDir, const std::filesystem::path &devicesRoot) {
  std::error_code ec;
  const auto rel = deviceDir.lexically_relative(devicesRoot).lexically_normal().generic_string();
  if (rel.empty() || rel == "." || rel.starts_with(".."))
    return false;

  // ACPI Lid Switch (e.g. platform/PNP0C0D:01)
  if (rel.starts_with("platform/PNP0C0D:") && rel.find('/', std::string("platform/").size()) == std::string::npos)
    return true;

  // Built-in i8042 keyboard controller port (e.g. platform/i8042/serio0 with description "i8042 KBD port")
  if (rel.starts_with("platform/i8042/serio") &&
      rel.find('/', std::string("platform/i8042/").size()) == std::string::npos) {
    const auto desc = readTrimmed(deviceDir / "description");
    if (desc.find("KBD") != std::string::npos)
      return true;
  }

  (void)ec;
  return false;
}

} // namespace wakeup_policy_detail

inline WakeupApplyResult applyWakeupPolicy(bool wakeOnlyLidAndKeyboard,
                                           const std::filesystem::path &sysRoot = "/sys",
                                           const std::filesystem::path &procRoot = "/proc") {
  WakeupApplyResult result;
  if (!wakeOnlyLidAndKeyboard)
    return result;

  const auto devicesRoot = sysRoot / "devices";
  std::error_code ec;
  if (std::filesystem::exists(devicesRoot, ec)) {
    std::filesystem::recursive_directory_iterator it(
        devicesRoot, std::filesystem::directory_options::skip_permission_denied, ec);
    const std::filesystem::recursive_directory_iterator end;
    while (!ec && it != end) {
      const auto &entry = *it;
      const auto path = entry.path();
      if (path.filename() == "wakeup" && path.parent_path().filename() == "power") {
        const auto current = wakeup_policy_detail::readTrimmed(path);
        if (current == "enabled" || current == "disabled") {
          const auto deviceDir = path.parent_path().parent_path();
          const bool allowed = wakeup_policy_detail::isAllowedWakeupDevice(deviceDir, devicesRoot);
          if (allowed) {
            if (current != "enabled") {
              if (wakeup_policy_detail::writeValue(path, "enabled"))
                ++result.enabled;
              else
                ++result.failed;
            }
          } else if (current == "enabled") {
            if (wakeup_policy_detail::writeValue(path, "disabled"))
              ++result.disabled;
            else
              ++result.failed;
          }
        }
      }
      it.increment(ec);
      if (ec) {
        ec.clear();
        break;
      }
    }
  }

  const auto acpiWakeupPath = procRoot / "acpi/wakeup";
  if (std::filesystem::exists(acpiWakeupPath, ec)) {
    std::ifstream in(acpiWakeupPath);
    if (in) {
      std::vector<std::string> toDisable;
      std::set<std::string> seen;
      std::string line;
      while (std::getline(in, line)) {
        std::istringstream iss(line);
        std::string dev, sstate, status;
        if (iss >> dev >> sstate >> status) {
          if (status.starts_with("*enabled") && seen.insert(dev).second)
            toDisable.push_back(std::move(dev));
        }
      }
      in.close();
      for (const auto &dev : toDisable) {
        std::ofstream out(acpiWakeupPath);
        if (!out)
          break;
        out << dev;
        out.flush();
        if (out)
          ++result.disabled;
      }
    }
  }

  return result;
}
