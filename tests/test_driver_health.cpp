// SPDX-License-Identifier: GPL-3.0-or-later
#include <QtTest>
#include <QTemporaryDir>
#include "DriverHealth.hpp"
class TestDriverHealth:public QObject {
 Q_OBJECT
 void node(const QString &root,const QString &name,const QByteArray &text={}) {QDir().mkpath(QFileInfo(root+"/"+name).absolutePath());QFile f(root+"/"+name);QVERIFY(f.open(QIODevice::WriteOnly));f.write(text);}
private slots:
 void missingModuleAndStaleBackendAreReported() {
  QTemporaryDir root;
  auto health=ucc::inspectDriverHealth(root.path());QVERIFY(!health["healthy"].toBool());QVERIFY(health["message"].toString().contains("DKMS"));
  QDir().mkpath(root.path()+"/sys/module/tuxedo_io");
  QVERIFY(!ucc::inspectDriverHealth(root.path())["healthy"].toBool());
  node(root.path(),"dev/tuxedo_io");QVERIFY(ucc::inspectDriverHealth(root.path(),true)["healthy"].toBool());
  health=ucc::inspectDriverHealth(root.path(),false);QVERIFY(!health["healthy"].toBool());QVERIFY(health["message"].toString().contains("перезапустите"));
 }
 void exactMechrevoRequiresKeyboardAndLightbarButGenericDoesNot() {
  QTemporaryDir root;QDir().mkpath(root.path()+"/sys/module/tuxedo_io");node(root.path(),"dev/tuxedo_io");
  node(root.path(),"sys/class/dmi/id/board_name","YAOSHI Series-X6AR55xY\n");
  QCOMPARE(ucc::inspectDriverHealth(root.path())["issues"].toArray().size(),2);
  QDir().mkpath(root.path()+"/sys/class/leds/rgb:kbd_backlight_0");
  QCOMPARE(ucc::inspectDriverHealth(root.path())["issues"].toArray().size(),1);
  node(root.path(),"sys/class/leds/rgb:lightbar/multi_intensity","0 0 0");
  QVERIFY(ucc::inspectDriverHealth(root.path())["healthy"].toBool());
  node(root.path(),"sys/class/dmi/id/board_name","Generic laptop");QDir(root.path()+"/sys/class/leds").removeRecursively();
  QVERIFY(ucc::inspectDriverHealth(root.path())["healthy"].toBool());
 }
};
QTEST_GUILESS_MAIN(TestDriverHealth)
#include "test_driver_health.moc"
