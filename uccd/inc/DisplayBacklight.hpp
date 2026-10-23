// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "SysfsNode.hpp"
#include <filesystem>
#include <fstream>
#include <algorithm>
#include <cmath>
#include <optional>
#include <string>
namespace display_backlight {
namespace fs = std::filesystem;
inline std::string read(const fs::path &path) {
  std::ifstream file(path); std::string value; file >> value; return value;
}
inline bool internalConnector(const fs::path &path) {
  const auto name = path.filename().string();
  return name.find("-eDP-") != std::string::npos || name.find("-LVDS-") != std::string::npos || name.find("-DSI-") != std::string::npos;
}
// Unknown topology remains usable for older ACPI/platform drivers. Explicitly
// disconnected internal panels must never win over the active hybrid-GPU panel.
inline int connectorScore(const fs::path &path) {
  if (read(path / "status") == "disconnected" || read(path / "enabled") == "disabled") return -1;
  if (read(path / "enabled") == "enabled") return 300;
  return read(path / "status") == "connected" ? 200 : 100;
}
inline int topologyScore(const fs::path &entry) {
  auto path = fs::canonical(entry);
  for (auto ancestor = path; ancestor != ancestor.root_path(); ancestor = ancestor.parent_path()) {
    if (internalConnector(ancestor)) return connectorScore(ancestor);
    if (fs::is_directory(ancestor / "drm")) {
      int best = -1; bool found = false;
      for (const auto &card : fs::directory_iterator(ancestor / "drm")) {
        if (!card.is_directory()) continue;
        for (const auto &connector : fs::directory_iterator(card.path())) {
          if (internalConnector(connector.path())) { found = true; best = std::max(best, connectorScore(connector.path())); }
        }
      }
      if (found) return best;
      break;
    }
  }
  return 100;
}
struct Device { std::string path; int maximum; };
inline std::optional<Device> select(const fs::path &base = "/sys/class/backlight") {
  std::optional<Device> selected; int best = -1;
  std::error_code ec;
  for (const auto &entry : fs::directory_iterator(base, ec)) {
    try {
      const auto maximum = SysfsNode<int>((entry.path()/"max_brightness").string()).read();
      if (!maximum || *maximum <= 0 || !SysfsNode<int>((entry.path()/"brightness").string()).read()) continue;
      const int topology = topologyScore(entry.path());
      if (topology < 0) continue;
      const auto type = read(entry.path()/"type");
      const int score = topology * 10 + (type == "raw" ? 2 : type == "platform" ? 1 : 0);
      const auto path = entry.path().string();
      if (score > best || (score == best && score >= 0 && selected && path < selected->path)) {
        best = score; selected = Device{path, *maximum};
      }
    } catch (const std::exception &) { /* disappearing sysfs device */ }
  }
  return selected;
}
}
/**
 * @brief Controller for display backlight brightness via sysfs
 *
 * Reads and writes brightness through the selected sysfs backlight interface.
 * Includes workaround for amdgpu_bl driver brightness inversion.
 */
class DisplayBacklightController
{
public:
  explicit DisplayBacklightController( const std::string &basePath, int maxBrightness, bool isAmdgpuBl );

  [[nodiscard]] int getBrightness() const noexcept;
  bool setBrightness( int brightness ) noexcept;
  [[nodiscard]] const std::string &getDriverName() const noexcept { return m_driverName; }

private:
  std::string m_basePath;
  std::string m_driverName;
  int m_maxBrightness;
  bool m_isAmdgpuBl;
};


inline DisplayBacklightController::DisplayBacklightController( const std::string &basePath,
                                                        int maxBrightness,
                                                        bool isAmdgpuBl )
  : m_basePath( basePath ), m_maxBrightness( maxBrightness ), m_isAmdgpuBl( isAmdgpuBl )
{
  // Extract driver name from path
  m_driverName = std::filesystem::path( basePath ).filename().string();
}

inline int DisplayBacklightController::getBrightness() const noexcept
{
  SysfsNode< int > brightnessNode( m_basePath + "/brightness" );
  auto value = brightnessNode.read();

  if ( not value.has_value() )
    return -1;

  int rawBrightness = value.value();

  // amdgpu_bl has inverted brightness: 0 = max, max = off
  if ( m_isAmdgpuBl )
    rawBrightness = m_maxBrightness - rawBrightness;

  // Convert to percentage
  if ( m_maxBrightness <= 0 )
    return -1;

  return static_cast<int>(std::lround(100.0 * rawBrightness / m_maxBrightness));
}

inline bool DisplayBacklightController::setBrightness( int brightness ) noexcept
{
  if ( m_maxBrightness <= 0 )
    return false;

  // Convert percentage to raw value
  int rawBrightness = static_cast<int>(std::lround(std::clamp(brightness, 0, 100) * m_maxBrightness / 100.0));
  rawBrightness = std::clamp( rawBrightness, 0, m_maxBrightness );

  // amdgpu_bl: 0 = max, max = off (inverted)
  if ( m_isAmdgpuBl )
    rawBrightness = m_maxBrightness - rawBrightness;

  std::ofstream file(m_basePath + "/brightness");
  file << rawBrightness << std::flush;
  return static_cast<bool>(file);
}

