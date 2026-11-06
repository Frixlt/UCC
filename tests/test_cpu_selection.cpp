// SPDX-License-Identifier: GPL-3.0-or-later
#include <QtTest>
#include <QTemporaryDir>
#include "CpuController.hpp"
#include "CpuTopology.hpp"
#include "ProfileManager.hpp"
#include "workers/CpuWorker.hpp"
#include <fstream>

class TestCpuSelection:public QObject {
  Q_OBJECT
  using Path=std::filesystem::path;
  static void file(const Path& p,const std::string& value) {std::filesystem::create_directories(p.parent_path());std::ofstream(p)<<value;}
  static std::string read(const Path& p) {std::string value;std::ifstream(p)>>value;return value;}
  static void fixture(const Path& root) {
    file(root/"present","0-1,8-9");file(root/"possible","0-1,8-9");file(root/"online","0-1");
    file(root/"intel_pstate/no_turbo","1");file(root/"intel_pstate/hwp_dynamic_boost","0");
    for(int id:{0,1,8,9}) {
      const auto p=root/("cpu"+std::to_string(id));if(id) file(p/"online",id==1 ? "1" : "0");
      const auto freq=p/"cpufreq";
      file(freq/"cpuinfo_min_freq","800000");file(freq/"cpuinfo_max_freq",id<8 ? "5400000" : "3600000");
      file(freq/"base_frequency",id<8 ? "2700000" : "2100000");
      file(freq/"scaling_min_freq","800000");file(freq/"scaling_max_freq","2700000");
      file(freq/"scaling_governor","performance");file(freq/"scaling_available_governors","performance powersave");
      file(freq/"energy_performance_preference","performance");file(freq/"energy_performance_available_preferences","performance power");
      file(freq/"scaling_driver","intel_pstate");
    }
  }
private slots:
  void masksRoundTripAndRejectMalformedIds() {
    auto p=ProfileManager::parseProfileJSON(R"({"id":"test","name":"Test","cpu":{"onlineCores":3,"enabledCores":[9,0,8]}})");
    QVERIFY(p.cpu.enabledCores);QCOMPARE(*p.cpu.enabledCores,std::vector<int32_t>({0,8,9}));
    auto again=ProfileManager::parseProfileJSON(ProfileManager::profileToJSON(p));QCOMPARE(*again.cpu.enabledCores,*p.cpu.enabledCores);
    QVERIFY(!ProfileManager::parseProfileJSON(R"({"id":"old","cpu":{"onlineCores":2}})").cpu.enabledCores);
    for(const auto* mask:{"[]","[0,0]","[-1,0]","[0,1.5]","[0,\"8\"]","null","[0,99999999999]"}) {
      bool rejected=false;
      try {(void)ProfileManager::parseProfileJSON(std::string("{\"id\":\"test\",\"cpu\":{\"enabledCores\":")+mask+"}}");} catch(const std::exception&) {rejected=true;}
      QVERIFY2(rejected,mask);
    }
  }
  void selectedIdsEnableBeforeDisablingAndPreserveMandatoryCpu() {
    QTemporaryDir temp;const Path root=temp.path().toStdString();fixture(root);CpuController c(root.string());
    std::vector<std::pair<int,bool>> writes;std::string error;
    QVERIFY(c.setEnabledCores({0,8,9},error,[&](auto& core,bool enabled){writes.emplace_back(core.coreIndex,enabled);return core.online.write(enabled);}));
    const std::vector<std::pair<int,bool>> expected{{8,true},{9,true},{1,false}};QCOMPARE(writes,expected);
    QCOMPARE(read(root/"cpu8/online"),std::string("1"));QCOMPARE(read(root/"cpu1/online"),std::string("0"));
    for(const auto& ids:std::vector<std::vector<int32_t>>{{},{8,9},{0,999},{0,8,8}}) QVERIFY(!c.setEnabledCores(ids,error));
    c.useCores(2);QCOMPARE(read(root/"cpu1/online"),std::string("1"));QCOMPARE(read(root/"cpu8/online"),std::string("0"));
  }
  void failedEnableDoesNotDisableExistingCpu() {
    QTemporaryDir temp;const Path root=temp.path().toStdString();fixture(root);CpuController c(root.string());std::string error;
    QVERIFY(!c.setEnabledCores({0,8},error,[](auto&,bool){return false;}));
    QCOMPARE(read(root/"cpu1/online"),std::string("1"));
  }
  void sparseWorkerConfigurationSurvivesValidationAndRestoresAllCpus() {
    QTemporaryDir temp;const Path root=temp.path().toStdString();fixture(root);
    UccProfile p("test","Test");p.cpu.enabledCores=std::vector<int32_t>{0,8,9};p.cpu.onlineCores=2;
    p.cpu.noTurbo=true;p.cpu.governor="powersave";p.cpu.energyPerformancePreference="power";
    p.cpu.scalingMinFrequency=800000;p.cpu.scalingMaxFrequency=2700000;
    std::vector<std::string> messages;
    CpuWorker worker([&]{return p;},[]{return true;},[&](const auto& message){messages.push_back(message);},root.string());
    worker.onStart();
    QCOMPARE(read(root/"cpu1/online"),std::string("0"));QCOMPARE(read(root/"cpu8/online"),std::string("1"));
    QCOMPARE(read(root/"cpu8/cpufreq/scaling_governor"),std::string("powersave"));
    QCOMPARE(read(root/"cpu9/cpufreq/energy_performance_preference"),std::string("power"));
    QCOMPARE(read(root/"cpu8/cpufreq/scaling_max_freq"),std::string("2100000"));
    messages.clear();worker.onWork();QVERIFY(messages.empty());
    // High minimum requests must also clamp to each core's non-turbo limit.
    p.cpu.scalingMinFrequency=2700000;worker.reapplyProfile();
    QCOMPARE(read(root/"cpu8/cpufreq/scaling_min_freq"),std::string("2100000"));
    messages.clear();worker.onWork();QVERIFY(messages.empty());
    worker.onExit();QCOMPARE(read(root/"cpu1/online"),std::string("1"));QCOMPARE(read(root/"cpu8/online"),std::string("1"));
    QCOMPARE(read(root/"intel_pstate/no_turbo"),std::string("0"));
  }
  void topologyIncludesOfflineIdsWithoutProbingOrEnablingThem() {
    QTemporaryDir temp;const Path root=temp.path().toStdString();fixture(root);CpuTopology topology(root.string());
    const auto rows=topology.read();QCOMPARE(rows.size(),size_t(4));QCOMPARE(rows[2]["id"].get<int>(),8);
    QVERIFY(!rows[2]["online"].get<bool>());QVERIFY(!rows[0]["hotpluggable"].get<bool>());
    QCOMPARE(read(root/"cpu8/online"),std::string("0"));
  }
};
QTEST_GUILESS_MAIN(TestCpuSelection)
#include "test_cpu_selection.moc"
