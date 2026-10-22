// SPDX-License-Identifier: GPL-3.0-or-later
#include <QtTest>
#include <QTemporaryDir>
#include <QProcess>
#include <QFile>
#include <QDir>
#include <QTextStream>
#include <QScopeGuard>
#include <signal.h>
#include "ProcessTerminator.hpp"
#include "PreviewMode.hpp"
class TestProcessTerminator:public QObject {
  Q_OBJECT
  QTemporaryDir root;
  void launch(QProcess &child,bool ignore=false) {
    child.start(QCoreApplication::applicationFilePath(),{"--child",root.path()+"/nvidia0",ignore ? "ignore" : "normal"});QVERIFY(child.waitForStarted());QVERIFY(child.waitForReadyRead());QCOMPARE(child.readAllStandardOutput().trimmed(),QByteArray("READY"));
  }
  ucc::SessionPaths paths() {ucc::SessionPaths p;p.nvidiaPrefix=root.path()+"/nvidia";return p;}
private slots:
  void gracefulTerminationAndRepeatedJobs() {
    QProcess child;launch(child);const auto cleanup=qScopeGuard([&]{if(child.state()!=QProcess::NotRunning){child.kill();child.waitForFinished();}});
    ucc::SessionReader reader(paths());const auto token=reader.processIdentity(child.processId());QVERIFY(!token.isEmpty());QVERIFY(reader.holdsNvidiaDevice(child.processId()));
    ucc::ProcessTerminator job(paths());QSignalSpy done(&job,&ucc::ProcessTerminator::finished);
#ifdef UCC_READ_ONLY_PREVIEW
    QVERIFY(!job.start({token}));QTRY_COMPARE(done.size(),1);QCOMPARE(child.state(),QProcess::Running);
#else
    QVERIFY(job.start({token}));QTRY_COMPARE(done.size(),1);QVERIFY(done[0][0].toString().contains("Exited: 1"));QVERIFY(done[0][0].toString().contains("Forced: 0"));QTRY_COMPARE(child.state(),QProcess::NotRunning);
    launch(child);QVERIFY(job.start({reader.processIdentity(child.processId())}));QTRY_COMPARE(done.size(),2);QTRY_COMPARE(child.state(),QProcess::NotRunning);
#endif
  }
  void staleIdentityNeverSignalsTheChild() {
    QProcess child;launch(child);const auto cleanup=qScopeGuard([&]{child.kill();child.waitForFinished();});ucc::SessionReader reader(paths());auto token=reader.processIdentity(child.processId());token["startTime"]="1";
    ucc::ProcessTerminator job(paths());QSignalSpy done(&job,&ucc::ProcessTerminator::finished);job.start({token});QTRY_COMPARE(done.size(),1);QCOMPARE(child.state(),QProcess::Running);
#ifndef UCC_READ_ONLY_PREVIEW
    QVERIFY(done[0][0].toString().contains("no longer matches"));
#endif
  }
  void unresponsiveChildIsKilledAfterGracePeriod() {
#ifdef UCC_READ_ONLY_PREVIEW
    QSKIP("Preview cannot terminate processes");
#endif
    QProcess child;launch(child,true);const auto cleanup=qScopeGuard([&]{if(child.state()!=QProcess::NotRunning){child.kill();child.waitForFinished();}});ucc::SessionReader reader(paths());ucc::ProcessTerminator job(paths());QSignalSpy done(&job,&ucc::ProcessTerminator::finished);QElapsedTimer elapsed;elapsed.start();QVERIFY(job.start({reader.processIdentity(child.processId())}));QVERIFY(!job.start({reader.processIdentity(child.processId())}));QTRY_COMPARE_WITH_TIMEOUT(done.size(),1,6000);QVERIFY(elapsed.elapsed()>=2800);QVERIFY(done[0][0].toString().contains("Forced: 1"));QTRY_COMPARE(child.state(),QProcess::NotRunning);
  }
};
int main(int argc,char **argv) {
  QCoreApplication app(argc,argv);
  if(app.arguments().contains("--child")) {
    QFile device(app.arguments().value(2));if(!device.open(QIODevice::ReadWrite)) return 2;
    if(app.arguments().value(3)=="ignore") signal(SIGTERM,SIG_IGN);
    QTextStream(stdout)<<"READY\n"<<Qt::flush;return app.exec();
  }
  TestProcessTerminator test;return QTest::qExec(&test,argc,argv);
}
#include "test_process_terminator.moc"
