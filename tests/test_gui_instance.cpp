#include <QtTest>
#include <QApplication>
#include <QElapsedTimer>
#include <QProcess>
#include <QTemporaryDir>
#include <QTimer>
#include "GuiInstance.hpp"
#include <QDBusConnection>
#include <QDBusConnectionInterface>
#include <QDBusMessage>
#include <QDBusVirtualObject>

namespace {
class MinimalUccdStub : public QDBusVirtualObject {
public:
  QString introspect(const QString &) const override {
    return QStringLiteral("<interface name=\"com.uniwill.uccd\"/>");
  }
  bool handleMessage(const QDBusMessage &msg, const QDBusConnection &bus) override {
    const QString m = msg.member();
    QVariant reply;
    if (m == QStringLiteral("IsDeviceSupported")) {
      reply = true;
    } else if (m == QStringLiteral("GetSettingsJSON") || m == QStringLiteral("GetActiveProfileJSON")) {
      reply = QStringLiteral("{}");
    } else if (m.endsWith(QStringLiteral("JSON")) || m.endsWith(QStringLiteral("Available")) ||
               m.endsWith(QStringLiteral("Thresholds")) || m == QStringLiteral("GetFanProfileNames")) {
      reply = QStringLiteral("[]");
    } else if (m.endsWith(QStringLiteral("Supported")) || m.startsWith(QStringLiteral("Is")) ||
               m.startsWith(QStringLiteral("GetWebcam")) || m.startsWith(QStringLiteral("GetFnLock"))) {
      reply = false;
    } else if (m.startsWith(QStringLiteral("GetCurrent")) || m == QStringLiteral("GetPowerState") ||
               m == QStringLiteral("GetPrimeState") || m == QStringLiteral("GetDeviceName") ||
               m == QStringLiteral("UccdVersion")) {
      reply = QString();
    } else {
      reply = 0;
    }
    bus.send(msg.createReply(reply));
    return true;
  }
};
} // namespace

class InstanceTest : public QObject {
  Q_OBJECT
  QProcess primary;
  void launch(QProcess &process) {
    process.start(QCoreApplication::applicationFilePath(), {"--instance"});
  }
private slots:
  void init() {
    launch(primary);
    QVERIFY(primary.waitForStarted());
    QVERIFY(primary.waitForReadyRead());
    QVERIFY(primary.readAllStandardOutput().contains("READY"));
  }
  void launchTokenIsForwardedToTheExistingWindow() {
    QProcess second;auto env=QProcessEnvironment::systemEnvironment();env.insert("XDG_ACTIVATION_TOKEN","unit-test-token");second.setProcessEnvironment(env);
    launch(second);QVERIFY(second.waitForStarted());QVERIFY(second.waitForFinished(4000));QCOMPARE(second.exitCode(),0);
    QVERIFY(second.readAllStandardOutput().contains("FORWARDED"));
    QVERIFY(primary.waitForReadyRead());QVERIFY(primary.readAllStandardOutput().contains("RESTORED"));
  }
  void repeatedLaunchRestoresTheExistingWindow() {
    QProcess second;
    launch(second);
    QVERIFY(second.waitForStarted());
    const bool exited = second.waitForFinished(4000);
    if (!exited) { second.terminate(); second.waitForFinished(); }
    QVERIFY2(exited, "A repeated launch must exit, not open another GUI instance");
    QCOMPARE(second.exitCode(), 0);
    QVERIFY(second.readAllStandardOutput().contains("FORWARDED"));
    QVERIFY(primary.waitForReadyRead());
    QVERIFY2(primary.readAllStandardOutput().contains("RESTORED"),
             "The existing minimized window must become visible");
  }
  void concurrentLaunchesDoNotCreateMoreWindows() {
    QProcess a, b, c;
    for (auto *p : {&a,&b,&c}) launch(*p);
    for (auto *p : {&a,&b,&c}) {
      const bool exited = p->waitForFinished(4000);
      if (!exited) { p->terminate(); p->waitForFinished(); }
      QVERIFY2(exited, "Concurrent launch must use the existing owner");
      QCOMPARE(p->exitCode(), 0);
      QVERIFY(p->readAllStandardOutput().contains("FORWARDED"));
    }
  }
  void previewUsesSeparateSessionIdentity() {
#ifdef UCC_READ_ONLY_PREVIEW
    auto bus=QDBusConnection::sessionBus();
    QVERIFY2(bus.registerService("com.uniwill.UccGui"),"Prototype must not claim or activate the normal GUI service");
    QVERIFY(bus.interface()->isServiceRegistered("com.uniwill.UccGuiPreview").value());
    bus.unregisterService("com.uniwill.UccGui");
#else
    QSKIP("Preview build only");
#endif
  }
  void ownerExitAllowsANewWindow() {
    primary.terminate(); QVERIFY(primary.waitForFinished());
    launch(primary);
    QVERIFY(primary.waitForStarted());
    QVERIFY(primary.waitForReadyRead());
    QVERIFY(primary.readAllStandardOutput().contains("READY"));
  }
  void realGuiMinimizedExitsUnderRustRuntimeAndCloseTerminatesProcess() {
    // Release the --instance dummy owner so the real ucc-gui binary can own com.uniwill.UccGui.
    primary.terminate();
    QVERIFY(primary.waitForFinished(4000));

    const QString guiBin = QCoreApplication::applicationDirPath() + QStringLiteral("/ucc-gui");
    QVERIFY2(QFile::exists(guiBin), "ucc-gui binary must be built alongside test_gui_instance");

    QTemporaryDir homeDir;
    QVERIFY(homeDir.isValid());
    auto sessionBus = QDBusConnection::sessionBus();
    QProcess uccdStub;
    uccdStub.start(QCoreApplication::applicationFilePath(), {QStringLiteral("--serve-uccd")});
    QVERIFY(uccdStub.waitForStarted(4000));
    QVERIFY(uccdStub.waitForReadyRead(4000));
    QVERIFY(uccdStub.readAllStandardOutput().contains("READY"));

    QProcessEnvironment env = QProcessEnvironment::systemEnvironment();
    env.insert(QStringLiteral("HOME"), homeDir.path());
    env.insert(QStringLiteral("XDG_CONFIG_HOME"), homeDir.path() + QStringLiteral("/.config"));
    env.insert(QStringLiteral("DBUS_SYSTEM_BUS_ADDRESS"), qEnvironmentVariable("DBUS_SESSION_BUS_ADDRESS"));
    env.insert(QStringLiteral("QT_QPA_PLATFORM"), QStringLiteral("offscreen"));

#ifdef UCC_PACKAGE_RUST_RUNTIME
    {
      QProcess minimizedGui;
      minimizedGui.setProcessEnvironment(env);
      minimizedGui.start(guiBin, {QStringLiteral("--minimized")});
      QVERIFY(minimizedGui.waitForStarted(4000));
      QVERIFY2(minimizedGui.waitForFinished(5000),
               "ucc-gui --minimized under UCC_PACKAGE_RUST_RUNTIME must exit (0 MB resident)");
      QCOMPARE(minimizedGui.exitCode(), 0);
      QVERIFY(!sessionBus.interface()->isServiceRegistered(QStringLiteral("com.uniwill.UccGui")).value());
    }
#endif

    // Launch real ucc-gui --show and measure startup latency until its event loop answers Activate.
    QElapsedTimer timer;
    timer.start();
    QProcess realGui;
    realGui.setProcessEnvironment(env);
    realGui.start(guiBin, {QStringLiteral("--show")});
    QVERIFY(realGui.waitForStarted(4000));
    QTRY_VERIFY_WITH_TIMEOUT(
        [&] {
          auto act = QDBusMessage::createMethodCall(QStringLiteral("com.uniwill.UccGui"),
                                                    QStringLiteral("/com/uniwill/UccGui"),
                                                    QStringLiteral("com.uniwill.UccGui"),
                                                    QStringLiteral("Activate"));
          return sessionBus.call(act, QDBus::Block, 500).type() == QDBusMessage::ReplyMessage;
        }(),
        8000);
    const qint64 startupMs = timer.elapsed();
    qInfo() << "M10.4 real ucc-gui --show ready latency (ms):" << startupMs;

    // Repeating launch with XDG_ACTIVATION_TOKEN activates existing owner and exits cleanly.
    QProcess second;
    QProcessEnvironment secondEnv = env;
    secondEnv.insert(QStringLiteral("XDG_ACTIVATION_TOKEN"), QStringLiteral("real-gui-token-1"));
    second.setProcessEnvironment(secondEnv);
    second.start(guiBin, {QStringLiteral("--show")});
    QVERIFY(second.waitForStarted(4000));
    QVERIFY(second.waitForFinished(5000));
    QVERIFY2(second.exitCode() == 0,
             qPrintable(QString::fromUtf8(second.readAllStandardError())));

    // Exiting primary terminates the Qt process completely and frees the bus name for relaunch.
    realGui.terminate();
    QVERIFY(realGui.waitForFinished(5000));
    QCOMPARE(realGui.state(), QProcess::NotRunning);
    QTRY_VERIFY_WITH_TIMEOUT(
        !sessionBus.interface()->isServiceRegistered(QStringLiteral("com.uniwill.UccGui")).value(),
        4000);

    QProcess relaunched;
    relaunched.setProcessEnvironment(env);
    relaunched.start(guiBin, {QStringLiteral("--show")});
    QVERIFY(relaunched.waitForStarted(4000));
    QTRY_VERIFY_WITH_TIMEOUT(
        [&] {
          auto act = QDBusMessage::createMethodCall(QStringLiteral("com.uniwill.UccGui"),
                                                    QStringLiteral("/com/uniwill/UccGui"),
                                                    QStringLiteral("com.uniwill.UccGui"),
                                                    QStringLiteral("Activate"));
          return sessionBus.call(act, QDBus::Block, 500).type() == QDBusMessage::ReplyMessage;
        }(),
        8000);
    relaunched.terminate();
    QVERIFY(relaunched.waitForFinished(5000));

    uccdStub.terminate();
    QVERIFY(uccdStub.waitForFinished(4000));
  }
  void cleanup() { primary.terminate(); primary.waitForFinished(); }
};

int main(int argc,char **argv) {
  QApplication app(argc,argv);
  if (app.arguments().contains("--serve-uccd")) {
    MinimalUccdStub stub;
    auto bus = QDBusConnection::sessionBus();
    if (!bus.registerService(QStringLiteral("com.uniwill.uccd")) ||
        !bus.registerVirtualObject(QStringLiteral("/com/uniwill/uccd"), &stub)) {
      return 3;
    }
    QTextStream(stdout) << "READY\n" << Qt::flush;
    return app.exec();
  }
  if (app.arguments().contains("--instance")) {
    ucc::GuiInstance instance;
    const auto result=instance.startOrActivate();
    if (result==ucc::GuiInstance::Result::ActivatedExisting) {
      QTextStream(stdout)<<"FORWARDED\n"<<Qt::flush; return 0;
    }
    if (result!=ucc::GuiInstance::Result::Primary) return 2;
    QWidget window;
    instance.setWindow(&window);
    window.showMinimized();
    QTextStream(stdout)<<"READY\n"<<Qt::flush;
    QTimer observed;
    QObject::connect(&observed,&QTimer::timeout,&window,[&] {
      if (window.isVisible() && !window.isMinimized()) {
        QTextStream(stdout)<<"RESTORED\n"<<Qt::flush;
        observed.stop();
      }
    });
    observed.start(10);
    return app.exec();
  }
  InstanceTest test;
  return QTest::qExec(&test,argc,argv);
}
#include "test_gui_instance.moc"
