// SPDX-License-Identifier: GPL-3.0-or-later
#include <QtTest>
#include "../ucc-session/NotificationMonitor.hpp"
class TestNotificationMonitor:public QObject {
  Q_OBJECT
private slots:
  void onlyMethodHeadersTriggerAndInputIsBounded() {
    int calls=0;NotificationMonitor monitor([&]{++calls;});
    const QByteArray header="method call time=1 sender=:1.2 -> destination=:1.3 serial=4 path=/org/freedesktop/Notifications; interface=org.freedesktop.Notifications; member=Notify\n";
    monitor.feed("  string \"method call interface=org.freedesktop.Notifications; member=Notify\"\n");QCOMPARE(calls,0);
    monitor.feed(header.left(50));QCOMPARE(calls,0);monitor.feed(header.mid(50));QCOMPARE(calls,1);
    monitor.feed(header);QCOMPARE(calls,1); // storm coalescing
    QTest::qWait(110);monitor.feed(QByteArray(5000,'x')+header);QCOMPARE(calls,1);
    monitor.feed(header);QCOMPARE(calls,2);
    QVERIFY(!NotificationMonitor::isNotification("signal interface=org.freedesktop.Notifications; member=Notify"));
    QVERIFY(!NotificationMonitor::isNotification("method call interface=org.freedesktop.Notifications; member=CloseNotification"));
  }
};
QTEST_GUILESS_MAIN(TestNotificationMonitor)
#include "test_notification_monitor.moc"
