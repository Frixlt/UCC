#include "GuiAudit.hpp"

void GuiAudit::initTestCase() {
    daemon.start(QCoreApplication::applicationFilePath(),{"--serve"});
    QVERIFY(daemon.waitForStarted());QVERIFY(daemon.waitForReadyRead());
    QCOMPARE(daemon.readAllStandardOutput().trimmed(),QByteArray("READY"));
    client=std::make_unique<ucc::UccdClient>();
    profiles=std::make_unique<ucc::ProfileManager>();
  }
void GuiAudit::init() { control("TestReset"); ucc::FluentTheme::setMode(ucc::FluentTheme::Mode::Light,false); }
void GuiAudit::cleanupTestCase() { profiles.reset();client.reset();if(daemon.state()!=QProcess::NotRunning){daemon.terminate();daemon.waitForFinished();} }
int main(int argc,char **argv) {
  QApplication app(argc,argv);
  ucc::FluentTheme::apply(app);
  if(app.arguments().contains("--serve")) {
    AuditDaemon service;auto bus=QDBusConnection::systemBus();
    if(!bus.registerVirtualObject("/com/uniwill/uccd",&service)||!bus.registerService("com.uniwill.uccd"))return 2;
    QTextStream(stdout)<<"READY\n"<<Qt::flush;return app.exec();
  }
  GuiAudit test;return QTest::qExec(&test,argc,argv);
}
