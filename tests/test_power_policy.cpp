// SPDX-License-Identifier: GPL-3.0-or-later
#include <QTest>
#include <QTemporaryDir>
#include "PowerPolicy.hpp"
#include "SettingsManager.hpp"

class TestPowerPolicy : public QObject {
  Q_OBJECT
  static void supply(const QString &root, const char *name, const char *type, int online) {
    const auto path = std::filesystem::path(root.toStdString())/name;
    std::filesystem::create_directories(path);
    std::ofstream(path/"type") << type;
    std::ofstream(path/"online") << online;
  }
private slots:
  void allSuppliesAreConsidered() {
    QTemporaryDir root;
    supply(root.path(),"AC0","Mains",0);
    supply(root.path(),"AC1","Mains",1);
    QCOMPARE(determineState(root.path().toStdString()),ProfileState::AC);
    supply(root.path(),"ucsi-source-psy-USBC000:001","USB",1);
    QCOMPARE(determineState(root.path().toStdString()),ProfileState::TYPE_C);
    supply(root.path(),"ucsi-source-psy-USBC000:001","USB",0);
    supply(root.path(),"AC1","Mains",0);
    QCOMPARE(determineState(root.path().toStdString()),ProfileState::BAT);
  }
  void usbPDDoesNotDependOnName() {
    QTemporaryDir root;
    supply(root.path(),"charger0","USB_PD",1);
    QCOMPARE(determineState(root.path().toStdString()),ProfileState::TYPE_C);
    QCOMPARE(profileStateToString(ProfileState::TYPE_C),std::string("power_type_c"));
  }
  void missingSuppliesPreserveDesktopDefault() {
    QTemporaryDir root;
    QCOMPARE(determineState(root.path().toStdString()),ProfileState::AC);
    supply(root.path(),"BAT0","Battery",0);
    QCOMPARE(determineState(root.path().toStdString()),ProfileState::BAT);
  }
  void manualWinsAndAutoRestoresCooler() {
    PowerPolicy policy;
    QVERIFY(policy.registerRequest(":1.1","render","Rendering","ac"));
    QVERIFY(policy.setManualMode("bat"));
    const auto manual = policy.evaluate(ProfileState::TYPE_C,true);
    QCOMPARE(manual.source,ProfileState::TYPE_C);
    QCOMPARE(manual.effective,ProfileState::BAT);
    QCOMPARE(manual.profileState,ProfileState::BAT);
    QVERIFY(policy.setManualMode("auto"));
    QCOMPARE(policy.evaluate(ProfileState::TYPE_C,true).profileState,ProfileState::AC);
    policy.removeOwner(":1.1");
    const auto automatic = policy.evaluate(ProfileState::TYPE_C,true);
    QCOMPARE(automatic.effective,ProfileState::TYPE_C);
    QCOMPARE(automatic.profileState,ProfileState::WC);
    QCOMPARE(automatic.status["real_power_source"].get<std::string>(),std::string("type_c"));
  }
  void requestsHaveOrderIndependentPriority() {
    PowerPolicy first,second;
    QVERIFY(first.registerRequest(":1.1","a","","type_c"));
    QVERIFY(first.registerRequest(":1.2","b","","ac"));
    QVERIFY(second.registerRequest(":1.1","a","","ac"));
    QVERIFY(second.registerRequest(":1.2","b","","type_c"));
    QCOMPARE(first.evaluate(ProfileState::BAT,false).effective,ProfileState::AC);
    QCOMPARE(second.evaluate(ProfileState::BAT,false).effective,ProfileState::AC);
    first.removeOwner(":1.2");
    QCOMPARE(first.evaluate(ProfileState::BAT,false).effective,ProfileState::TYPE_C);
    QCOMPARE(first.evaluate(ProfileState::AC,false).effective,ProfileState::AC);
  }
  void ownershipAndValidation() {
    PowerPolicy policy;
    QVERIFY(policy.registerRequest(":1.1","same","","ac"));
    QVERIFY(!policy.unregisterRequest(":1.2","same"));
    QVERIFY(!policy.setManualMode("power_wc"));
    QVERIFY(!policy.registerRequest("","a","","ac"));
    QVERIFY(!policy.registerRequest(":1.2","a","","invalid"));
    QCOMPARE(policy.evaluate(ProfileState::BAT,false).effective,ProfileState::AC);
    QVERIFY(policy.unregisterRequest(":1.1","same"));
    QCOMPARE(policy.evaluate(ProfileState::BAT,false).effective,ProfileState::BAT);
  }
  void informationalClientsDoNotOverrideCooler() {
    PowerPolicy policy;
    QVERIFY(policy.registerRequest(":1.1","lightbar","Notifications",""));
    QCOMPARE(policy.evaluate(ProfileState::AC,true).profileState,ProfileState::WC);
    QVERIFY(policy.evaluate(ProfileState::AC,true).status["registered_apps"].is_array());
  }
  void settingsKeepTypeCAndPersonalLayout() {
    SettingsManager manager;
    const auto result = manager.parseSettingsJSON(R"({"keyboardLayout":"frixit-mechrevo","stateMap":{"power_ac":"mains","power_type_c":"usb","power_bat":"battery","power_wc":"cooler"}})");
    QVERIFY(result.has_value());
    QCOMPARE(result->stateMap.at("power_type_c"),std::string("usb"));
    QCOMPARE(result->keyboardLayout,std::string("frixit-mechrevo"));
    const auto colors=manager.parseSettingsJSON(R"({"keyboardBacklightStates":[{"red":1,"green":2,"blue":3,"brightness":17}],"powerDevices":{"enabled":true,"devices":[]}})");
    QVERIFY(colors.has_value());
    QCOMPARE(nlohmann::json::parse(colors->keyboardBacklightStates)[0]["red"].get<int>(),1);
    QVERIFY(nlohmann::json::parse(colors->powerDeviceSettings)["enabled"].get<bool>());
    const auto old = manager.parseSettingsJSON(R"({"stateMap":{"power_ac":"mains"}})");
    QVERIFY(old.has_value());
    QCOMPARE(old->keyboardLayout,std::string("auto"));
    QVERIFY(!old->stateMap.contains("power_type_c"));
  }
};
QTEST_GUILESS_MAIN(TestPowerPolicy)
#include "test_power_policy.moc"
