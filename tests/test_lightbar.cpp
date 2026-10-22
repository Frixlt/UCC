// SPDX-License-Identifier: GPL-3.0-or-later
#include <QtTest>
#include <QTemporaryDir>
#include "LightbarController.hpp"
#include "SettingsManager.hpp"
class TestLightbar:public QObject {
  Q_OBJECT
  using Controller=LightbarController;
  using Json=Controller::Json;
  void file(const QString &root,const QString &name,const QByteArray &data) {QDir().mkpath(QFileInfo(root+"/"+name).absolutePath());QFile f(root+"/"+name);QVERIFY(f.open(QIODevice::WriteOnly));f.write(data);}
  QByteArray read(const QString &root,const QString &name) {QFile f(root+"/"+name);if(!f.open(QIODevice::ReadOnly)) return {};return f.readAll();}
  void hardware(const QString &root) {file(root,"rgb:lightbar/multi_intensity","1 2 3 1 2 3");file(root,"rgb:lightbar/brightness","0");file(root,"rgb:lightbar/max_brightness","50");}
private slots:
  void resumeKeepsWelcomeSettingWithoutReplayingIt() {
    QTemporaryDir root;hardware(root.path());Controller c(root.path());auto config=Controller::defaults();
    config["managed"]=true;config["welcome_anim"]=true;c.skipWelcome();QVERIFY(c.configure(config));
    QVERIFY(c.settings()["welcome_anim"].get<bool>());QVERIFY(!c.status()["animating"].get<bool>());
    QVERIFY(c.trigger(true));
  }
  void disabledByDefaultAndSettingsParse() {
    QTemporaryDir root;hardware(root.path());Controller c(root.path());QVERIFY(c.configure(Json::object()));
    QCOMPARE(read(root.path(),"rgb:lightbar/multi_intensity"),QByteArray("1 2 3 1 2 3"));QVERIFY(!c.trigger(true));
    SettingsManager manager;auto settings=manager.parseSettingsJSON("{\"lightbar\":{\"managed\":true,\"silent_mode\":true}}");QVERIFY(settings);
    const auto json=Json::parse(settings->lightbarSettings);QVERIFY(json["managed"].get<bool>());QVERIFY(json["silent_mode"].get<bool>());
  }
  void sourceColumnsBrightnessAndCancellation() {
    QTemporaryDir root;hardware(root.path());Controller c(root.path());auto config=Controller::defaults();config["managed"]=true;
    config["power_settings"]["ac"]["color_mode"]={20,40,60};config["power_settings"]["type_c"]["color_mode"]={1,2,3};config["power_settings"]["type_c"]["brightness_val"]=148;
    QVERIFY(c.configure(config));QCOMPARE(read(root.path(),"rgb:lightbar/brightness"),QByteArray("50"));
    QVERIFY(c.trigger());c.advance(820);QCOMPARE(read(root.path(),"rgb:lightbar/multi_intensity"),QByteArray("0 0 0 0 0 0"));
    c.setPowerMode("type_c");QVERIFY(!c.status()["animating"].get<bool>());QCOMPARE(read(root.path(),"rgb:lightbar/brightness"),QByteArray("29"));QCOMPARE(read(root.path(),"rgb:lightbar/multi_intensity"),QByteArray("1 2 3 1 2 3"));
    c.setPowerMode("bat");QVERIFY(!c.trigger(true));QCOMPARE(read(root.path(),"rgb:lightbar/multi_intensity"),QByteArray("0 0 0 0 0 0"));
  }
  void autoUsesMajorityNonBlackIncludingLedZero() {
    QTemporaryDir root;hardware(root.path());
    file(root.path(),"rgb:kbd_backlight/multi_intensity","9 8 7");file(root.path(),"rgb:kbd_backlight_21/multi_intensity","9 8 7");file(root.path(),"rgb:kbd_backlight_56/multi_intensity","1 2 3");
    for(int i=0;i<4;++i) file(root.path(),QString("rgb:kbd_backlight_%1/multi_intensity").arg(i+70),"0 0 0");
    Controller c(root.path());auto config=Controller::defaults();config["managed"]=true;QVERIFY(c.configure(config));
    QCOMPARE(c.color(),(Controller::Color{9,8,7}));QCOMPARE(read(root.path(),"rgb:lightbar/multi_intensity"),QByteArray("9 8 7 9 8 7"));
    file(root.path(),"rgb:kbd_backlight_56/multi_intensity","9 8 7");file(root.path(),"rgb:kbd_backlight/multi_intensity","4 5 6");c.keyboardChanged();QCOMPARE(c.color(),(Controller::Color{9,8,7}));
  }
  void animationEndStateSilentWelcomeAndNoOverlap() {
    QTemporaryDir root;hardware(root.path());Controller c(root.path());auto config=Controller::defaults();config["managed"]=true;config["silent_mode"]=true;config["welcome_anim"]=true;config["power_settings"]["ac"]["color_mode"]={80,90,100};
    QVERIFY(c.configure(config));QVERIFY(c.status()["animating"].get<bool>());QVERIFY(!c.trigger(true));c.advance(5000);QVERIFY(!c.trigger());
    for(const auto *animation:Controller::animations) {
      config["power_settings"]["ac"]["mode"]=animation;QVERIFY(c.configure(config));QVERIFY(!c.status()["animating"].get<bool>());QVERIFY(c.trigger(true));c.advance(Controller::duration(animation));
      const auto expected=std::string(animation)=="Flash (stays off)" ? QByteArray("0 0 0 0 0 0") : QByteArray("80 90 100 80 90 100");QCOMPARE(read(root.path(),"rgb:lightbar/multi_intensity"),expected);
      c.keyboardChanged();QCOMPARE(read(root.path(),"rgb:lightbar/multi_intensity"),expected);
    }
  }
  void animationFramesMatchTheOriginalPatterns() {
    const Controller::Color base={100,80,60};
    QCOMPARE(Controller::frame("Pulse (stays on)",0,base),base);
    QCOMPARE(Controller::frame("Pulse (stays on)",820,base),(Controller::Color{0,0,0}));
    QCOMPARE(Controller::frame("Flash (stays off)",0,base),(Controller::Color{0,0,0}));
    QCOMPARE(Controller::frame("Flash (stays off)",820,base),base);
    QCOMPARE(Controller::frame("Police Strobe",0,base),(Controller::Color{255,0,0}));
    QCOMPARE(Controller::frame("Police Strobe",60,base),(Controller::Color{0,0,0}));
    QCOMPARE(Controller::frame("Police Strobe",120,base),(Controller::Color{0,0,255}));
    QVERIFY(Controller::frame("Rainbow",0,base)!=Controller::frame("Rainbow",1000,base));
    QCOMPARE(Controller::frame("Disco",60,base),(Controller::Color{0,0,0}));
    QVERIFY(Controller::frame("Disco",0,base)!=Controller::frame("Disco",90,base));
  }
  void failuresAndInvalidSettingsDoNotClaimSuccess() {
    QTemporaryDir root;Controller c(root.path());auto config=Controller::defaults();config["managed"]=true;QVERIFY(c.configure(config));QVERIFY(!c.trigger(true));QVERIFY(!c.status()["ok"].get<bool>());
    Json normalized;std::string error;
    for(const auto &profile: {Json{{"brightness_val",256}},Json{{"mode","bad"}},Json{{"color_mode",{1,2,-1}}},Json{{"enabled","yes"}}})
      QVERIFY(!Controller::normalize({{"power_settings",{{"ac",profile}}}},normalized,error));
    hardware(root.path());QVERIFY(c.configure(config));QVERIFY(QFile::remove(root.path()+"/rgb:lightbar/multi_intensity"));QVERIFY(!c.trigger(true));
  }
};
QTEST_GUILESS_MAIN(TestLightbar)
#include "test_lightbar.moc"
