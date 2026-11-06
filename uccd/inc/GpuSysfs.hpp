// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "SysfsNode.hpp"
#include <filesystem>
#include <algorithm>

namespace ucc {
struct IntelGpuSample {
  double frequencyMHz=-1, maxFrequencyMHz=-1, temperatureC=-1, powerW=-1;
};
inline IntelGpuSample readIntelGpuSysfs(const std::filesystem::path& card) {
  IntelGpuSample result;
  const auto read=[](const std::filesystem::path& p) {return SysfsNode<int64_t>(p.string()).read().value_or(-1);};
  // i915 uses DRM card attributes. xe exposes the render GT under its PCI device.
  auto frequency=card/"gt_act_freq_mhz", maximum=card/"gt_RP0_freq_mhz";
  std::error_code ec;
  if(!std::filesystem::exists(frequency,ec)) {
    frequency=card/"device/tile0/gt0/freq0/act_freq";
    maximum=card/"device/tile0/gt0/freq0/rp0_freq";
  }
  result.frequencyMHz=static_cast<double>(read(frequency));
  result.maxFrequencyMHz=static_cast<double>(read(maximum));
  // Only use sensors belonging to this GPU. CPU package temperature is not a GPU sensor.
  ec.clear();
  for(const auto& h:std::filesystem::directory_iterator(card/"device/hwmon",ec)) {
    const auto temp=read(h.path()/"temp1_input");
    if(temp>=0) result.temperatureC=static_cast<double>(temp)/1000.0;
    auto power=read(h.path()/"power1_average");
    if(power<0) power=read(h.path()/"power1_input");
    if(power>=0) result.powerW=static_cast<double>(power)/1000000.0;
  }
  return result;
}
inline std::string intelGpuRaplPath(const std::filesystem::path& root="/sys/class/powercap") {
  std::error_code ec;
  for(const auto& entry:std::filesystem::directory_iterator(root,ec)) {
    const auto name=SysfsNode<std::string>((entry.path()/"name").string()).read().value_or("");
    if(name=="uncore" || name=="gpu" || name=="graphics") return entry.path().string();
  }
  return {};
}
inline bool nvidiaRuntimeActive(const std::filesystem::path& root="/sys/bus/pci/devices") {
  std::error_code ec;
  for(const auto& entry:std::filesystem::directory_iterator(root,ec)) {
    const auto base=entry.path();
    if(SysfsNode<std::string>((base/"vendor").string()).read().value_or("")!="0x10de") continue;
    const auto cls=SysfsNode<std::string>((base/"class").string()).read().value_or("");
    if(!cls.starts_with("0x03")) continue;
    // Unknown, suspending and suspended states must never trigger an NVML probe.
    if(SysfsNode<std::string>((base/"power/runtime_status").string()).read().value_or("")=="active") return true;
  }
  return false;
}
inline bool nvidiaTelemetryDue(unsigned cycle,const std::filesystem::path& root="/sys/bus/pci/devices") {
  std::error_code ec;
  for(const auto& entry:std::filesystem::directory_iterator(root,ec)) {
    const auto base=entry.path();
    if(SysfsNode<std::string>((base/"vendor").string()).read().value_or("")!="0x10de") continue;
    if(!SysfsNode<std::string>((base/"class").string()).read().value_or("").starts_with("0x03")) continue;
    if(SysfsNode<std::string>((base/"power/runtime_status").string()).read().value_or("")!="active") continue;
    // NVML itself resets the driver's roughly ten-second idle timeout. Leave
    // a full idle window when runtime suspend is allowed (20 x 800 ms = 16 s).
    return SysfsNode<std::string>((base/"power/control").string()).read().value_or("")=="on" || cycle%20==0;
  }
  return false;
}
}
