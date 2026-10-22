// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include "profiles/UccProfile.hpp"
#include <nlohmann/json.hpp>
#include <filesystem>
#include <fstream>
#include <functional>
#include <map>
#include <mutex>
#include <regex>
#include <set>
#include <algorithm>
#include <tuple>
#include <cctype>

// All paths below are kernel interfaces with specific allowed values. D-Bus
// clients cannot supply an arbitrary root-writable path or systemd service.
class PowerDeviceController {
public:
  using Json = nlohmann::json;
  using Runner = std::function<bool(const std::string &,const std::vector<std::string> &)>;
  explicit PowerDeviceController(std::filesystem::path sys = "/sys",
    std::filesystem::path proc = "/proc", Runner runner = {},
    std::filesystem::path programs = "/usr/bin")
    : m_sys(std::move(sys)),m_proc(std::move(proc)),m_programs(std::move(programs)),m_runner(std::move(runner)) {}

  void setRunner(Runner runner) {m_runner=std::move(runner);}

  static bool validate(const Json &settings, std::string &error) {
    try {
      if (!settings.is_object()) throw std::runtime_error("Settings must be an object");
      if (settings.contains("enabled") && !settings["enabled"].is_boolean()) throw std::runtime_error("Invalid enabled flag");
      if (settings.contains("keepNvidiaAwake") && !settings["keepNvidiaAwake"].is_boolean()) throw std::runtime_error("Invalid NVIDIA policy");
      const auto cpu=settings.value("cpu",Json::object());
      if (!cpu.is_object()) throw std::runtime_error("Invalid CPU policy");
      for (const auto &[mode,policy] : cpu.items()) {
        if (!validMode(mode) || !policy.is_object()) throw std::runtime_error("Invalid CPU mode");
        for (const auto &[field,value] : policy.items()) {
          if (field=="governor" && oneOf(value,{"","performance","powersave","ondemand","conservative","schedutil","userspace"})) continue;
          if (field=="epp" && oneOf(value,{"","default","performance","balance_performance","balance_power","power"})) continue;
          if (field=="aspm" && oneOf(value,{"","default","performance","powersave","powersupersave"})) continue;
          if (field=="noTurbo" && value.is_boolean()) continue;
          throw std::runtime_error("Invalid CPU field: "+field);
        }
      }
      const auto devices=settings.value("devices",Json::array());
      if (!devices.is_array() || devices.size()>2048) throw std::runtime_error("Invalid device list");
      std::set<std::string> paths;
      for (const auto &device : devices) {
        if (!device.is_object()) throw std::runtime_error("Invalid device entry");
        const auto type=device.at("type").get<std::string>();
        const auto path=device.at("path").get<std::string>();
        if (!validPath(type,path) || !paths.insert(path).second) throw std::runtime_error("Invalid or duplicate device: "+path);
        if (!device.at("name").is_string() || device["name"].get<std::string>().size()>512) throw std::runtime_error("Invalid device name");
        for (const auto *mode : {"ac","type_c","bat"}) {
          const auto &value=device.at(std::string(mode)+"_val");
          if (!value.is_string() || !validValue(type,path,value.get<std::string>())) throw std::runtime_error("Invalid value for "+path);
        }
      }
      return true;
    } catch (const std::exception &exception) { error=exception.what(); return false; }
  }

  bool setSettings(const Json &settings, std::string &error) {
    if (!validate(settings,error)) return false;
    std::lock_guard lock(m_mutex);
    m_settings=settings;
    ++m_revision;
    return true;
  }
  Json settings() const {std::lock_guard lock(m_mutex);return m_settings;}
  Json status() const {std::lock_guard lock(m_mutex);return m_status;}
  uint64_t revision() const {std::lock_guard lock(m_mutex);return m_revision;}

  void overlayCpu(UccProfile &profile, const std::string &mode) const {
    const auto config=settings();
    if (!config.value("enabled",false)) return;
    const auto cpu=config.value("cpu",Json::object()).value(mode,Json::object());
    if (!cpu.value("governor",std::string()).empty()) profile.cpu.governor=cpu["governor"];
    if (!cpu.value("epp",std::string()).empty()) profile.cpu.energyPerformancePreference=cpu["epp"];
    if (cpu.contains("noTurbo")) profile.cpu.noTurbo=cpu["noTurbo"];
    for (const auto &device : config.value("devices",Json::array()))
      if (device["path"]=="/sys/devices/system/cpu/intel_pstate/no_turbo")
        profile.cpu.noTurbo=device[mode+"_val"]=="1";
  }

  void apply(const std::string &mode) {
    if (!validMode(mode)) return;
    const auto config=settings();
    Json result={{"mode",mode},{"enabled",config.value("enabled",false)},{"devices",Json::array()}};
    if (config.value("enabled",false)) {
      const auto cpu=config.value("cpu",Json::object()).value(mode,Json::object());
      const std::string aspm=cpu.value("aspm",std::string());
      if (!aspm.empty()) result["aspmApplied"]=write("/sys/module/pcie_aspm/parameters/policy",aspm);
      bool wakeNvidia=false;
      for(const auto &device:config.value("devices",Json::array())) {
        if(device["type"]!="pci") continue;
        const std::string path=device["path"];
        const auto base=std::filesystem::path(path).parent_path().parent_path();
        if(read(base/"vendor")=="0x10de" && read(base/"class").starts_with("0x03") &&
           (config.value("keepNvidiaAwake",false) || device[mode+"_val"]=="on")) {
          wakeNvidia=write(path,"on") || wakeNvidia;
        }
      }
      if(wakeNvidia && std::filesystem::exists(m_proc/"driver/nvidia/version"))
        result["nvidiaPersistenceApplied"]=run("nvidia-smi",{"-pm","1"});
      for (const auto &device : config.value("devices",Json::array())) {
        const std::string type=device["type"], path=device["path"];
        std::string value=device[mode+"_val"];
        bool ok=false, guarded=false;
        if (type=="wifi") ok=run("iw",{"dev",path,"set","power_save",value});
        else if (type=="service") ok=run("systemctl",{value=="active" ? "start" : "stop","nvidia-powerd.service"});
        else {
          if (type=="pci" && config.value("keepNvidiaAwake",false)) {
            const auto base=std::filesystem::path(path).parent_path().parent_path();
            if (read(base/"vendor")=="0x10de" && read(base/"class").starts_with("0x03")) {
              value="on";guarded=true;wakeNvidia=true;
            }
          }
          ok=write(path,value);
        }
        result["devices"].push_back({{"path",path},{"value",value},{"ok",ok},{"guarded",guarded}});
      }
      result["cpu"]={{"governor",read("/sys/devices/system/cpu/cpu0/cpufreq/scaling_governor")},
        {"epp",read("/sys/devices/system/cpu/cpu0/cpufreq/energy_performance_preference")},
        {"noTurbo",read("/sys/devices/system/cpu/intel_pstate/no_turbo")}};
    }
    std::lock_guard lock(m_mutex);m_status=std::move(result);
  }

  Json scan() const {
    auto config=settings();
    if (!config.contains("cpu")) config["cpu"]=Json::object();
    auto devices=config.value("devices",Json::array());
    std::set<std::string> existing;
    for (const auto &device:devices) existing.insert(device["path"]);
    auto append=[&](const std::string &type,const std::string &path,const std::string &name,
                    const std::string &ac,const std::string &battery) {
      if (existing.insert(path).second)
        devices.push_back({{"type",type},{"path",path},{"name",name},
          {"ac_val",ac},{"type_c_val",battery},{"bat_val",battery}});
    };
    auto enumerate=[&](const char *bus) {
      const auto directory=m_sys/(std::string("bus/")+bus+"/devices");
      std::error_code error;
      for (const auto &entry:std::filesystem::directory_iterator(directory,error)) {
        if (!std::filesystem::exists(entry.path()/"power/control",error)) continue;
        const auto id=entry.path().filename().string();
        const auto path="/sys/bus/"+std::string(bus)+"/devices/"+id+"/power/control";
        std::string name=std::string(bus)+" "+id;
        if (std::string(bus)=="usb") {
          const auto product=read(entry.path()/"manufacturer")+" "+read(entry.path()/"product");
          if (product!=" ") name=product;
        } else name+=" ("+read(entry.path()/"vendor")+":"+read(entry.path()/"device")+")";
        auto lower=name;std::transform(lower.begin(),lower.end(),lower.begin(),[](unsigned char c){return static_cast<char>(std::tolower(c));});
        const bool critical=std::string(bus)=="usb" && (lower.find("keyboard")!=std::string::npos || lower.find("mouse")!=std::string::npos || lower.find("bluetooth")!=std::string::npos || lower.find("receiver")!=std::string::npos || lower.find("controller")!=std::string::npos);
        if (validPath(bus,path)) append(bus,path,name,"on",critical ? "on" : "auto");
      }
    };
    enumerate("pci");enumerate("usb");
    std::error_code error;
    for (const auto &entry:std::filesystem::directory_iterator(m_sys/"class/net",error))
      if (std::filesystem::exists(entry.path()/"wireless",error))
        append("wifi",entry.path().filename().string(),"Wi-Fi power save ("+entry.path().filename().string()+")","off","on");
    if (std::filesystem::exists(m_programs/"nvidia-powerd")) append("service","nvidia-powerd","NVIDIA Dynamic Boost","active","inactive");
    const std::vector<std::tuple<std::string,std::string,std::string,std::string>> system={
      {"Audio Powersave","/sys/module/snd_hda_intel/parameters/power_save","0","1"},
      {"NMI Watchdog","/proc/sys/kernel/nmi_watchdog","0","0"},
      {"VM Writeback","/proc/sys/vm/dirty_writeback_centisecs","500","1500"},
      {"CPU Turbo Boost (1 = disabled)","/sys/devices/system/cpu/intel_pstate/no_turbo","0","1"}};
    for (const auto &[name,path,ac,battery]:system)
      if (std::filesystem::exists(mapped(path),error)) append("sys",path,name,ac,battery);
    config["devices"]=devices;
    return config;
  }

private:
  static bool validMode(const std::string &mode) {return mode=="ac" || mode=="type_c" || mode=="bat";}
  static bool oneOf(const Json &value,std::initializer_list<const char *> values) {
    if (!value.is_string()) return false;
    for (const auto *allowed:values) if (value==allowed) return true;
    return false;
  }
  static bool validPath(const std::string &type,const std::string &path) {
    if (type=="pci") return std::regex_match(path,std::regex(R"(/sys/bus/pci/devices/[0-9a-fA-F]{4}:[0-9a-fA-F]{2}:[0-9a-fA-F]{2}\.[0-7]/power/control)"));
    if (type=="usb") return path.find("..") == std::string::npos && std::regex_match(path,std::regex(R"(/sys/bus/usb/devices/[A-Za-z0-9_.:-]+/power/control)"));
    if (type=="wifi") return path.find("..") == std::string::npos && std::regex_match(path,std::regex("[A-Za-z0-9_.:-]{1,32}"));
    if (type=="service") return path=="nvidia-powerd" || path=="nvidia-powerd.service";
    if (type=="sys") return path=="/sys/module/snd_hda_intel/parameters/power_save" ||
      path=="/proc/sys/kernel/nmi_watchdog" || path=="/proc/sys/vm/dirty_writeback_centisecs" ||
      path=="/sys/devices/system/cpu/intel_pstate/no_turbo";
    return false;
  }
  static bool validValue(const std::string &type,const std::string &path,const std::string &value) {
    if (type=="pci" || type=="usb") return value=="on" || value=="auto";
    if (type=="wifi") return value=="on" || value=="off";
    if (type=="service") return value=="active" || value=="inactive";
    if (value.empty() || value.size()>6 || value.find_first_not_of("0123456789")!=std::string::npos) return false;
    const int number=std::stoi(value);
    if (path=="/sys/module/snd_hda_intel/parameters/power_save") return number<=3600;
    if (path=="/proc/sys/vm/dirty_writeback_centisecs") return number<=60000;
    return number<=1;
  }
  std::filesystem::path mapped(const std::filesystem::path &path) const {
    const auto text=path.string();
    if (text.starts_with("/sys/")) return m_sys/text.substr(5);
    if (text.starts_with("/proc/")) return m_proc/text.substr(6);
    return path;
  }
  std::string read(const std::filesystem::path &path) const {
    std::ifstream file(mapped(path));std::string value;std::getline(file,value);return value;
  }
  bool write(const std::string &path,const std::string &value) const {
    const auto target=mapped(path);
    std::error_code error;
    if (!std::filesystem::exists(target,error)) return false;
    if (read(path)==value) return true;
    std::ofstream file(target);file<<value;file.flush();return file.good();
  }
  bool run(const std::string &program,const std::vector<std::string> &args) const {return m_runner && m_runner(program,args);}
  mutable std::mutex m_mutex;
  Json m_settings=Json::object(),m_status=Json::object();
  uint64_t m_revision=0;
  std::filesystem::path m_sys,m_proc,m_programs;
  Runner m_runner;
};
