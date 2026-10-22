// SPDX-License-Identifier: GPL-3.0-or-later
#include <QtTest>
#include <QDBusVirtualObject>
#include "DriverAlert.hpp"
class Notifications:public QDBusVirtualObject {
public:
 int count=0,closed=0;bool fail=false;QString body;
 QString introspect(const QString &) const override {return {};}
 bool handleMessage(const QDBusMessage &m,const QDBusConnection &bus) override {
  if(m.member()=="Notify") {++count;body=m.arguments()[4].toString();bus.send(fail ? m.createErrorReply(QDBusError::Failed,"Not ready") : m.createReply(QVariantList{quint32(count)}));return true;}
  if(m.member()=="CloseNotification") {++closed;bus.send(m.createReply());return true;}
  return false;
 }
};
class TestDriverAlert:public QObject {
 Q_OBJECT
 Notifications notifications;
private slots:
 void initTestCase() {auto bus=QDBusConnection::sessionBus();QVERIFY(bus.registerService("org.freedesktop.Notifications"));QVERIFY(bus.registerVirtualObject("/org/freedesktop/Notifications",&notifications));}
 void notifiesOnceClosesOnRecoveryAndAllowsNewFailure() {
  ucc::DriverAlert alert;const QJsonObject broken{{"healthy",false},{"message","Module unavailable for kernel"}},healthy{{"healthy",true}};
  alert.update(healthy);QTest::qWait(20);QCOMPARE(notifications.count,0);
  alert.update(broken);alert.update(broken);QTRY_COMPARE(notifications.count,1);QCOMPARE(notifications.body,QString("Module unavailable for kernel"));
  QTest::qWait(30);alert.update(broken);QTest::qWait(20);QCOMPARE(notifications.count,1);
  alert.update(healthy);QTRY_COMPARE(notifications.closed,1);
  alert.update(broken);QTRY_COMPARE(notifications.count,2);
 }
 void retriesWhenNotificationServiceRejectedTheFirstAttempt() {
  ucc::DriverAlert alert;QJsonObject broken{{"healthy",false},{"message","Driver failure"}};
  const int before=notifications.count;notifications.fail=true;alert.update(broken);QTRY_COMPARE(notifications.count,before+1);QTest::qWait(30);
  notifications.fail=false;alert.update(broken);QTRY_COMPARE(notifications.count,before+2);
 }
};
QTEST_GUILESS_MAIN(TestDriverAlert)
#include "test_driver_alert.moc"
