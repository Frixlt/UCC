// SPDX-License-Identifier: GPL-3.0-or-later
#include "PreviewMode.hpp"
#include "RefreshAdvisor.hpp"
#include <QDBusArgument>
#include <QDBusMessage>
#include <QDBusVirtualObject>
#include <QJsonArray>
#include <QSettings>
#include <QtTest>
using namespace ucc;
namespace
{
QJsonObject mode( QString id, double hz, int w = 2560, int h = 1600 )
{
  return { { "id", id }, { "refreshRate", hz }, { "size", QJsonObject{ { "width", w }, { "height", h } } } };
}
QJsonObject configuration( QString current = "fast", QString name = "eDP-1" )
{
  return { { "outputs", QJsonArray{ QJsonObject{
                            { "name", name },
                            { "enabled", true },
                            { "connected", true },
                            { "currentModeId", current },
                            { "modes", QJsonArray{ mode( "fast", 240 ), mode( "slow", 60 ),
                                                   mode( "wrong", 60, 1600, 1200 ), mode( "ultra", 300 ) } } } } } };
}
QString policy( QString source, QString manual = "auto" )
{
  return QString::fromUtf8(
      QJsonDocument( QJsonObject{ { "real_power_source", source }, { "manual_mode", manual } } ).toJson() );
}
bool write( const QString& path, const QByteArray& bytes )
{
  QFile file( path );
  return file.open( QIODevice::WriteOnly | QIODevice::Truncate ) && file.write( bytes ) == bytes.size();
}
class Notifications : public QDBusVirtualObject
{
public:
  uint next = 0;
  QList< uint > closed;
  QList< QVariantList > offers;
  QString introspect( const QString& ) const override
  {
    return {};
  }
  bool handleMessage( const QDBusMessage& m, const QDBusConnection& bus ) override
  {
    if( m.member() == "Notify" )
    {
      offers.append( m.arguments() );
      bus.send( m.createReply( QVariantList{ ++next } ) );
      return true;
    }
    if( m.member() == "CloseNotification" )
    {
      closed.append( m.arguments()[0].toUInt() );
      bus.send( m.createReply() );
      return true;
    }
    return false;
  }
  void action( uint id )
  {
    auto signal = QDBusMessage::createSignal( "/org/freedesktop/Notifications", "org.freedesktop.Notifications",
                                              "ActionInvoked" );
    signal.setArguments( { id, QString( "apply-refresh" ) } );
    QDBusConnection::sessionBus().send( signal );
  }
};
} // namespace
class TestRefreshAdvisor : public QObject
{
  Q_OBJECT
  QTemporaryDir directory;
  Notifications notifications;
  QString command, config, log;
  QByteArray logContents()
  {
    QFile f( log );
    return f.open( QIODevice::ReadOnly ) ? f.readAll() : QByteArray{};
  }
private slots:
  void initTestCase()
  {
    QVERIFY( directory.isValid() );
    command = directory.filePath( "kscreen" );
    config = directory.filePath( "configuration.json" );
    log = directory.filePath( "calls" );
    QVERIFY( write( command, R"PY(#!/usr/bin/python3
import json, sys
from pathlib import Path
root = Path(__file__).parent
with (root / 'calls').open('a') as log: log.write(' '.join(sys.argv[1:]) + '\n')
config = root / 'configuration.json'
data = json.loads(config.read_text())
if sys.argv[1:] == ['-j']: print(json.dumps(data))
else:
    assert len(sys.argv) == 2
    if (root / 'ignore-mode').exists(): sys.exit(0)
    selected = sys.argv[1].split('.')
    assert selected[:2] == ['output', 'eDP-1'] and selected[2] == 'mode'
    data['outputs'][0]['currentModeId'] = selected[3]
    config.write_text(json.dumps(data))
)PY" ) );
    QVERIFY( QFile::setPermissions( command, QFile::ReadOwner | QFile::WriteOwner | QFile::ExeOwner ) );
    auto bus = QDBusConnection::sessionBus();
    QVERIFY( bus.registerService( "org.freedesktop.Notifications" ) );
    QVERIFY( bus.registerVirtualObject( "/org/freedesktop/Notifications", &notifications ) );
  }
  void init()
  {
    QFile::remove( log );
    QFile::remove( directory.filePath( "ignore-mode" ) );
    notifications.offers.clear();
    notifications.closed.clear();
    QVERIFY( write( config, QJsonDocument( configuration() ).toJson() ) );
    QSettings( "UniwillControlCenter", "desktop" ).setValue( "refreshRateNotifications", true );
  }
  void modesKeepResolutionAndDiscoverNvidia()
  {
    auto bat = RefreshAdvisor::plan( configuration(), "bat", directory.path() );
    QVERIFY( bat );
    QCOMPARE( bat->targetMode, "slow" );
    QCOMPARE( bat->width, 2560 );
    QCOMPARE( bat->height, 1600 );
    auto ac = RefreshAdvisor::plan( configuration( "slow" ), "ac", directory.path() );
    QVERIFY( ac );
    QCOMPARE( ac->targetMode, "fast" );
    QVERIFY( QDir( directory.path() ).mkpath( "card7/device" ) );
    QVERIFY( QDir( directory.path() ).mkpath( "card7-eDP-1" ) );
    QVERIFY( write( directory.filePath( "card7/device/vendor" ), "0x10de\n" ) );
    ac = RefreshAdvisor::plan( configuration( "slow" ), "type_c", directory.path() );
    QVERIFY( ac );
    QCOMPARE( ac->targetMode, "ultra" );
    QVERIFY( !RefreshAdvisor::plan( configuration( "slow", "DP-1" ), "ac", directory.path() ) );
    QVERIFY( !RefreshAdvisor::plan( configuration( "slow" ), "bat", directory.path() ) );
    QVERIFY( !RefreshAdvisor::plan( configuration(), "invalid", directory.path() ) );
    auto unsupported = configuration();
    auto outputs = unsupported["outputs"].toArray();
    auto panel = outputs[0].toObject();
    panel["modes"] = QJsonArray{ mode( "fast", 240 ), mode( "wrong", 60, 1600, 1200 ) };
    outputs[0] = panel;
    unsupported["outputs"] = outputs;
    QVERIFY( !RefreshAdvisor::plan( unsupported, "bat", directory.path() ) );
    QDir( directory.filePath( "card7" ) ).removeRecursively();
    QDir( directory.filePath( "card7-eDP-1" ) ).removeRecursively();
  }
  void initialAndManualModesDoNotPrompt()
  {
    RefreshAdvisor advisor( true, command, directory.path() );
    advisor.policyChanged( policy( "ac" ) );
    advisor.policyChanged( policy( "ac", "bat" ) );
    advisor.policyChanged( "{}" );
    QTest::qWait( 950 );
    QVERIFY( logContents().isEmpty() );
    QVERIFY( notifications.offers.isEmpty() );
  }
  void appliesOnlyAfterActionAndVerifies()
  {
    if( readOnlyPreview )
      QSKIP( "Normal-build integration; preview guard is tested separately" );
    RefreshAdvisor advisor( true, command, directory.path() );
    QSignalSpy result( &advisor, &RefreshAdvisor::result );
    advisor.policyChanged( policy( "ac" ) );
    advisor.policyChanged( policy( "bat" ) );
    QTRY_COMPARE_WITH_TIMEOUT( notifications.offers.size(), 1, 4000 );
    QTest::qWait( 50 );
    QCOMPARE( logContents(), "-j\n" );
    notifications.action( notifications.next + 100 );
    QTest::qWait( 100 );
    QCOMPARE( logContents(), "-j\n" );
    notifications.action( notifications.next );
    QTRY_COMPARE_WITH_TIMEOUT( result.size(), 1, 4000 );
    QVERIFY( result[0][0].toBool() );
    QCOMPARE( logContents(), "-j\n-j\noutput.eDP-1.mode.slow\n-j\n" );
    QFile f( config );
    QVERIFY( f.open( QIODevice::ReadOnly ) );
    QVERIFY( RefreshAdvisor::applied( QJsonDocument::fromJson( f.readAll() ).object(),
                                      *RefreshAdvisor::plan( configuration(), "bat", directory.path() ) ) );
  }
  void changedDisplayDiscardsConsent()
  {
    if( readOnlyPreview )
      QSKIP( "Normal-build integration" );
    RefreshAdvisor advisor( true, command, directory.path() );
    QSignalSpy result( &advisor, &RefreshAdvisor::result );
    advisor.policyChanged( policy( "ac" ) );
    advisor.policyChanged( policy( "bat" ) );
    QTRY_COMPARE_WITH_TIMEOUT( notifications.offers.size(), 1, 4000 );
    QTest::qWait( 50 );
    QVERIFY( write( config, QJsonDocument( configuration( "slow" ) ).toJson() ) );
    notifications.action( notifications.next );
    QTRY_COMPARE_WITH_TIMEOUT( result.size(), 1, 2000 );
    QVERIFY( !result[0][0].toBool() );
    QVERIFY( !logContents().contains( "output." ) );
  }
  void successfulCommandWithoutModeChangeIsReportedAsFailure()
  {
    if( readOnlyPreview )
      QSKIP( "Normal-build integration" );
    QVERIFY( write( directory.filePath( "ignore-mode" ), "1" ) );
    RefreshAdvisor advisor( true, command, directory.path() );
    QSignalSpy result( &advisor, &RefreshAdvisor::result );
    advisor.policyChanged( policy( "ac" ) );
    advisor.policyChanged( policy( "bat" ) );
    QTRY_COMPARE_WITH_TIMEOUT( notifications.offers.size(), 1, 4000 );
    QTest::qWait( 50 );
    notifications.action( notifications.next );
    QTRY_COMPARE_WITH_TIMEOUT( result.size(), 1, 4000 );
    QVERIFY( !result[0][0].toBool() );
    QVERIFY( logContents().contains( "output.eDP-1.mode.slow" ) );
  }
  void sourceChangeAndSleepInvalidateActions()
  {
    if( readOnlyPreview )
      QSKIP( "Normal-build integration" );
    RefreshAdvisor advisor( true, command, directory.path() );
    advisor.policyChanged( policy( "ac" ) );
    advisor.policyChanged( policy( "bat" ) );
    QTRY_COMPARE_WITH_TIMEOUT( notifications.offers.size(), 1, 4000 );
    QTest::qWait( 50 );
    auto old = notifications.next;
    advisor.sleeping( true );
    notifications.action( old );
    QTest::qWait( 100 );
    QVERIFY( !logContents().contains( "output." ) );
    advisor.policyChanged( policy( "type_c" ) );
    advisor.sleeping( false );
    QTest::qWait( 1750 );
    QVERIFY( !logContents().contains( "output." ) );
    QCOMPARE( notifications.offers.size(), 1 ); // Already 240 Hz.
    advisor.policyChanged( policy( "bat" ) );
    QTRY_COMPARE_WITH_TIMEOUT( notifications.offers.size(), 2, 4000 );
    QTest::qWait( 50 );
    old = notifications.next;
    advisor.policyChanged( policy( "ac" ) );
    notifications.action( old );
    QTest::qWait( 150 );
    QVERIFY( !logContents().contains( "output." ) );
  }
  void disabledOrPreviewNeverExecutes()
  {
    QSettings( "UniwillControlCenter", "desktop" ).setValue( "refreshRateNotifications", false );
    RefreshAdvisor disabled( true, command, directory.path() ), otherDesktop( false, command, directory.path() );
    disabled.policyChanged( policy( "ac" ) );
    disabled.policyChanged( policy( "bat" ) );
    otherDesktop.policyChanged( policy( "ac" ) );
    otherDesktop.policyChanged( policy( "bat" ) );
    RefreshAdvisor preview( true, command, directory.path() );
    if( readOnlyPreview )
    {
      QSettings( "UniwillControlCenter", "desktop" ).setValue( "refreshRateNotifications", true );
      preview.policyChanged( policy( "ac" ) );
      preview.policyChanged( policy( "bat" ) );
    }
    QTest::qWait( 1000 );
    QVERIFY( logContents().isEmpty() );
    QVERIFY( notifications.offers.isEmpty() );
  }
  void loginSleepSignalsReachSessionAgent()
  {
    auto bus = QDBusConnection::systemBus();
    QVERIFY( bus.registerService( "org.freedesktop.login1" ) );
    SessionSleepMonitor monitor;
    QSignalSpy observed( &monitor, &SessionSleepMonitor::sleeping );
    auto signal =
        QDBusMessage::createSignal( "/org/freedesktop/login1", "org.freedesktop.login1.Manager", "PrepareForSleep" );
    signal.setArguments( { true } );
    QVERIFY( bus.send( signal ) );
    QTRY_COMPARE( observed.size(), 1 );
    QVERIFY( observed[0][0].toBool() );
    signal.setArguments( { false } );
    QVERIFY( bus.send( signal ) );
    QTRY_COMPARE( observed.size(), 2 );
    QVERIFY( !observed[1][0].toBool() );
    QVERIFY( bus.unregisterService( "org.freedesktop.login1" ) );
  }
};
QTEST_GUILESS_MAIN( TestRefreshAdvisor )
#include "test_refresh_advisor.moc"
