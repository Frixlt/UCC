// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QObject>
#include <QTimer>
#include <QElapsedTimer>
#include <QFile>
#include <QDir>
#include <QRegularExpression>
#include <nlohmann/json.hpp>
#include <array>
#include <map>
#include <cmath>
#include <algorithm>

// Confined to the main D-Bus thread. Animation frames never sleep or run a shell.
class LightbarController : public QObject {
public:
  using Json=nlohmann::json;
  using Color=std::array<int,3>;
  static inline const std::array<const char *,5> animations={
    "Pulse (stays on)","Flash (stays off)","Rainbow","Police Strobe","Disco"};
  explicit LightbarController(QString root="/sys/class/leds",QObject *parent=nullptr)
    : QObject(parent),m_root(std::move(root)) {
    discover();m_timer.setInterval(20);
    connect(&m_timer,&QTimer::timeout,this,[this]{advance(m_clock.elapsed());});
  }
  static Json defaults() {
    Json settings={{"managed",false},{"silent_mode",false},{"welcome_anim",false},{"power_settings",Json::object()}};
    for(const auto *mode:{"ac","type_c","bat"}) settings["power_settings"][mode]={
      {"enabled",std::string(mode)!="bat"},{"brightness_val",std::string(mode)=="ac" ? 255 : std::string(mode)=="type_c" ? 150 : 50},
      {"mode",animations[0]},{"color_mode","Auto"}};
    return settings;
  }
  static bool normalize(const Json &input,Json &result,std::string &error) {
    try {
      if(!input.is_object()) throw std::runtime_error("Lightbar settings must be an object");
      result=defaults();
      for(const auto *flag:{"managed","silent_mode","welcome_anim"}) if(input.contains(flag)) {
        if(!input[flag].is_boolean()) throw std::runtime_error("Invalid lightbar flag");
        result[flag]=input[flag];
      }
      if(input.contains("power_settings")) {
        if(!input["power_settings"].is_object()) throw std::runtime_error("Invalid lightbar power settings");
        for(const auto &[mode,profile]:input["power_settings"].items()) {
          if((mode!="ac" && mode!="type_c" && mode!="bat") || !profile.is_object()) throw std::runtime_error("Invalid lightbar power mode");
          auto &target=result["power_settings"][mode];
          if(profile.contains("enabled")) {
            if(!profile["enabled"].is_boolean()) throw std::runtime_error("Invalid enabled flag");
            target["enabled"]=profile["enabled"];
          }
          if(profile.contains("brightness_val")) {
            const auto &v=profile["brightness_val"];
            if(!v.is_number_integer() || v.get<int64_t>()<0 || v.get<int64_t>()>255) throw std::runtime_error("Brightness must be 0..255");
            target["brightness_val"]=v;
          }
          if(profile.contains("mode")) {
            const auto &v=profile["mode"];
            if(!v.is_string() || std::none_of(animations.begin(),animations.end(),[&](const char *a){return v==a;})) throw std::runtime_error("Invalid lightbar animation");
            target["mode"]=v;
          }
          if(profile.contains("color_mode")) {
            const auto &v=profile["color_mode"];
            if(v!="Auto") {
              if(!v.is_array() || v.size()!=3) throw std::runtime_error("Invalid lightbar color");
              for(const auto &c:v) if(!c.is_number_integer() || c.get<int64_t>()<0 || c.get<int64_t>()>255) throw std::runtime_error("Invalid RGB channel");
            }
            target["color_mode"]=v;
          }
        }
      }
      return true;
    } catch(const std::exception &e) {error=e.what();return false;}
  }
  bool configure(const Json &input) {
    Json normalized;std::string error;
    if(!normalize(input,normalized,error)) return false;
    const bool wasManaged=m_config.value("managed",false);
    m_timer.stop();m_animating=false;m_config=std::move(normalized);discover();
    if(m_config["managed"].get<bool>()) {
      applyIdle();
      if(!m_welcomeDone) {m_welcomeDone=true;if(m_config["welcome_anim"].get<bool>()) trigger(true);}
    } else if(wasManaged) {writeColor({0,0,0});}
    return true;
  }
  void shutdown() {auto config=m_config;config["managed"]=false;configure(config);}
  Json settings() const {return m_config;}
  Json status() const {return {{"available",!m_path.isEmpty()},{"managed",m_config["managed"]},
    {"mode",m_powerMode},{"animating",m_animating},{"zones",m_zones},
    {"maxBrightness",m_maxBrightness},{"ok",m_lastOk},{"error",m_error.toStdString()}};}
  void setPowerMode(const std::string &mode) {
    if(mode!="ac" && mode!="type_c" && mode!="bat") return;
    if(m_powerMode==mode) return;
    m_timer.stop();m_animating=false;m_powerMode=mode;
    if(m_config["managed"].get<bool>()) applyIdle();
  }
  void keyboardChanged() {
    if(m_config["managed"].get<bool>() && !m_animating && profile()["color_mode"]=="Auto" && !m_flashOff) applyIdle();
  }
  bool trigger(bool force=false) {
    if(!m_config["managed"].get<bool>() || !profile()["enabled"].get<bool>() || m_animating ||
       (!force && m_config["silent_mode"].get<bool>())) return false;
    discover();if(m_path.isEmpty()) {m_lastOk=false;m_error="Lightbar hardware unavailable";return false;}
    m_animation=profile()["mode"].get<std::string>();m_base=color();
    m_lastOk=true;m_error.clear();
    if(!writeBrightness()) return false;
    m_animating=true;m_flashOff=false;m_clock.start();advance(0);if(m_animating) m_timer.start();return m_lastOk;
  }
  // Public for deterministic fake-sysfs tests; normal operation is timer-driven.
  void advance(qint64 elapsed) {
    if(!m_animating) return;
    if(elapsed>=duration(m_animation)) {
      m_timer.stop();m_animating=false;m_flashOff=m_animation==animations[1];
      writeColor(m_flashOff ? Color{0,0,0} : color());return;
    }
    if(!writeColor(frame(m_animation,elapsed,m_base))) {m_timer.stop();m_animating=false;}
  }
  static qint64 duration(const std::string &animation) {
    if(animation==animations[2]) return 3000;
    if(animation==animations[3]) return 1440;
    if(animation==animations[4]) return 1350;
    return 1640;
  }
  static Color frame(const std::string &animation,qint64 elapsed,const Color &base) {
    if(animation==animations[2]) return hue(static_cast<double>(elapsed)/3000.0);
    if(animation==animations[3]) {const int phase=static_cast<int>(elapsed/60)%4;return phase==0 ? Color{255,0,0} : phase==2 ? Color{0,0,255} : Color{0,0,0};}
    if(animation==animations[4]) return elapsed%90>=60 ? Color{0,0,0} : hue(static_cast<double>((elapsed/90*137+41)%360)/360.0);
    const double pulse=std::abs(1.0-static_cast<double>(elapsed)/820.0);
    const double factor=std::clamp(animation==animations[1] ? 1.0-pulse : pulse,0.0,1.0);
    return {static_cast<int>(base[0]*factor),static_cast<int>(base[1]*factor),static_cast<int>(base[2]*factor)};
  }
  Color color() const {
    const auto selected=profile()["color_mode"];
    if(selected.is_array()) return {selected[0].get<int>(),selected[1].get<int>(),selected[2].get<int>()};
    std::map<Color,int> counts;
    for(const auto &name:QDir(m_root).entryList({"rgb:kbd_backlight*"},QDir::Dirs|QDir::NoDotAndDotDot,QDir::Name)) {
      const auto values=read(m_root+"/"+name+"/multi_intensity").split(QRegularExpression("\\s+"),Qt::SkipEmptyParts);
      if(values.size()<3) continue;
      Color rgb{};bool valid=true;
      for(int i=0;i<3;++i) {bool ok=false;rgb[static_cast<size_t>(i)]=values[i].toInt(&ok);valid &= ok && rgb[static_cast<size_t>(i)]>=0 && rgb[static_cast<size_t>(i)]<=255;}
      if(valid) ++counts[rgb];
    }
    if(counts.empty()) return {255,255,255};
    if(counts.size()>1) counts.erase(Color{0,0,0});
    return std::max_element(counts.begin(),counts.end(),[](const auto &a,const auto &b){return a.second<b.second;})->first;
  }
private:
  Json profile() const {return m_config["power_settings"][m_powerMode];}
  static QString read(const QString &path) {QFile f(path);return f.open(QIODevice::ReadOnly) ? QString::fromUtf8(f.readAll()).trimmed() : QString();}
  void discover() {
    m_path.clear();
    auto names=QDir(m_root).entryList(QDir::Dirs|QDir::NoDotAndDotDot,QDir::Name);
    for(const auto &preferred:{QString("lightbar"),QString("rgb:lightbar")}) if(names.removeOne(preferred)) names.prepend(preferred);
    for(const auto &name:names) {
      if(name.contains("keyboard") || name.contains("kbd") || name.contains("input") || (!name.contains("lightbar") && !name.contains("front"))) continue;
      const auto path=m_root+"/"+name;
      const auto values=read(path+"/multi_intensity").split(QRegularExpression("\\s+"),Qt::SkipEmptyParts);
      bool ok=false;const int maximum=read(path+"/max_brightness").toInt(&ok);
      if(values.isEmpty() || values.size()%3 || values.size()>3072 || !ok || maximum<=0 || maximum>65535 || !QFile::exists(path+"/brightness")) continue;
      m_path=path;m_zones=static_cast<int>(values.size()/3);m_maxBrightness=maximum;break;
    }
  }
  bool write(const QString &path,const QByteArray &value) {
    QFile file(path);
    if(!file.open(QIODevice::WriteOnly|QIODevice::Truncate) || file.write(value)!=value.size() || !file.flush()) {
      m_lastOk=false;m_error="Could not write "+path;return false;
    }
    return true;
  }
  bool writeColor(const Color &rgb) {
    if(m_path.isEmpty()) {m_lastOk=false;m_error="Lightbar hardware unavailable";return false;}
    QByteArray value;
    for(int i=0;i<m_zones;++i) {if(i) value+=' ';for(int c=0;c<3;++c) {if(c) value+=' ';value+=QByteArray::number(rgb[static_cast<size_t>(c)]);}}
    return write(m_path+"/multi_intensity",value);
  }
  bool writeBrightness() {
    return write(m_path+"/brightness",QByteArray::number(static_cast<int>(std::lround(profile()["brightness_val"].get<int>()*m_maxBrightness/255.0))));
  }
  void applyIdle() {
    m_flashOff=false;m_lastOk=true;m_error.clear();
    if(m_path.isEmpty()) discover();
    if(m_path.isEmpty()) {m_lastOk=false;m_error="Lightbar hardware unavailable";return;}
    if(!writeBrightness()) return;
    writeColor(profile()["enabled"].get<bool>() ? color() : Color{0,0,0});
  }
  static Color hue(double h) {
    const double v=(h-std::floor(h))*6.0;const int sector=static_cast<int>(v);
    const int up=static_cast<int>((v-sector)*255),down=255-up;
    switch(sector) {case 0:return {255,up,0};case 1:return {down,255,0};case 2:return {0,255,up};case 3:return {0,down,255};case 4:return {up,0,255};default:return {255,0,down};}
  }
  QString m_root,m_path,m_error;
  QTimer m_timer;
  QElapsedTimer m_clock;
  Json m_config=defaults();
  std::string m_powerMode="ac",m_animation;
  Color m_base{255,255,255};
  int m_zones=0,m_maxBrightness=0;
  bool m_animating=false,m_flashOff=false,m_welcomeDone=false,m_lastOk=true;
};
