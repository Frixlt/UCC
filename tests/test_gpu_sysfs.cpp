// SPDX-License-Identifier: GPL-3.0-or-later
#include "GpuSysfs.hpp"
#include <QtTest>
#include <fstream>
class TestGpuSysfs:public QObject {
  Q_OBJECT
  static void file(const std::filesystem::path& p,const std::string& v) {
    std::filesystem::create_directories(p.parent_path());std::ofstream(p)<<v;
  }
private slots:
  void i915AndXeFrequencyAndMissingTemperature() {
    QTemporaryDir dir;const std::filesystem::path card=dir.path().toStdString();
    file(card/"gt_act_freq_mhz","350");file(card/"gt_RP0_freq_mhz","1900");
    auto sample=ucc::readIntelGpuSysfs(card);
    QCOMPARE(sample.frequencyMHz,350.0);QCOMPARE(sample.maxFrequencyMHz,1900.0);QCOMPARE(sample.temperatureC,-1.0);
    std::filesystem::remove(card/"gt_act_freq_mhz");std::filesystem::remove(card/"gt_RP0_freq_mhz");
    file(card/"device/tile0/gt0/freq0/act_freq","0");file(card/"device/tile0/gt0/freq0/rp0_freq","1900");
    file(card/"device/tile0/gt1/freq0/act_freq","1300");
    sample=ucc::readIntelGpuSysfs(card);
    QCOMPARE(sample.frequencyMHz,0.0);QCOMPARE(sample.maxFrequencyMHz,1900.0);
    file(card/"device/hwmon/hwmon4/temp1_input","42500");file(card/"device/hwmon/hwmon4/power1_average","700000");
    sample=ucc::readIntelGpuSysfs(card);QCOMPARE(sample.temperatureC,42.5);QCOMPARE(sample.powerW,0.7);
  }
  void raplUsesDomainNameRatherThanIndex() {
    QTemporaryDir dir;const std::filesystem::path root=dir.path().toStdString();
    file(root/"intel-rapl:0:1/name","core");file(root/"intel-rapl:1:3/name","uncore");
    QCOMPARE(ucc::intelGpuRaplPath(root),(root/"intel-rapl:1:3").string());
  }
  void sleepingGpuDoesNotPermitNvmlProbe() {
    QTemporaryDir dir;const std::filesystem::path root=dir.path().toStdString(),gpu=root/"0000:02:00.0";
    file(gpu/"vendor","0x10de");file(gpu/"class","0x030000");
    for(const auto& state:{"suspended","suspending","unknown"}) {
      file(gpu/"power/runtime_status",state);QVERIFY(!ucc::nvidiaRuntimeActive(root));
    }
    file(gpu/"power/runtime_status","active");QVERIFY(ucc::nvidiaRuntimeActive(root));
    file(gpu/"power/control","auto");
    QVERIFY(!ucc::nvidiaTelemetryDue(1,root));QVERIFY(!ucc::nvidiaTelemetryDue(19,root));
    QVERIFY(ucc::nvidiaTelemetryDue(20,root));
    file(gpu/"power/control","on");QVERIFY(ucc::nvidiaTelemetryDue(1,root));
    file(gpu/"power/runtime_status","suspended");QVERIFY(!ucc::nvidiaTelemetryDue(20,root));
  }
};
QTEST_GUILESS_MAIN(TestGpuSysfs)
#include "test_gpu_sysfs.moc"
