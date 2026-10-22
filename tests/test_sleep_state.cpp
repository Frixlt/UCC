// SPDX-License-Identifier: GPL-3.0-or-later
#include "SleepState.hpp"
#include <QtTest>
using namespace ucc;
class TestSleepState : public QObject
{
  Q_OBJECT
private slots:
  void everyManualModeAndCoolerFlagSurvivesSleep()
  {
    for(const auto *mode:{"auto","ac","type_c","bat"}) for(bool cooler:{false,true}) {
      QTemporaryDir dir;SleepStateStore store(dir.filePath("state"),::getuid());QString error;
      SleepOperations ops;ops.active=[](bool &value,QString &){value=true;return true;};
      ops.capture=[&](SleepState &state,QString &){state.manualMode=mode;state.coolerEnabled=cooler;return true;};
      ops.stop=[](QString &){return true;};
      ops.start=[&](QString &err){std::optional<SleepState> state;return store.load(state,err) && state && state->manualMode==mode && state->coolerEnabled==cooler;};
      QVERIFY(prepareSleep(store,ops,error));QVERIFY(resumeSleep(store,ops,error));
    }
  }
  void failedStopKeepsSnapshotForRecovery()
  {
    QTemporaryDir dir;SleepStateStore store(dir.filePath("state"),::getuid());QString error;
    SleepOperations ops;ops.active=[](bool &v,QString &){v=true;return true;};
    ops.capture=[](SleepState &s,QString &){s.manualMode="bat";return true;};
    ops.stop=[](QString &e){e="stop failed";return false;};int starts=0;
    ops.start=[&](QString &){++starts;return true;};
    QVERIFY(!prepareSleep(store,ops,error));std::optional<SleepState> saved;QVERIFY(store.load(saved,error));QVERIFY(saved);
    QVERIFY(resumeSleep(store,ops,error));QCOMPARE(starts,1);
  }
  void runningDaemonKeepsOverridesAndStopsBeforeResume()
  {
    QTemporaryDir dir;
    SleepStateStore store( dir.filePath( "snapshot.json" ), ::getuid() );
    QString error;
    QStringList calls;
    SleepOperations ops;
    ops.active = [&]( bool& v, QString& )
    {
      calls << "active";
      v = true;
      return true;
    };
    ops.capture = [&]( SleepState& v, QString& )
    {
      calls << "capture";
      v.manualMode = "bat";
      v.coolerEnabled = true;
      return true;
    };
    ops.stop = [&]( QString& err )
    {
      calls << "stop";
      std::optional< SleepState > saved;
      return store.load( saved, err ) && saved && saved->wasActive && saved->manualMode == "bat";
    };
    ops.start = [&]( QString& err )
    {
      calls << "start";
      std::optional< SleepState > saved;
      return store.load( saved, err ) && saved && saved->coolerEnabled;
    };
    QVERIFY2( prepareSleep( store, ops, error ), qPrintable( error ) );
    QCOMPARE( calls, QStringList( { "active", "capture", "stop" } ) );
    std::optional< SleepState > state;
    QVERIFY( store.load( state, error ) );
    QVERIFY( state );
    QCOMPARE( state->manualMode, "bat" );
    QCOMPARE( QFileInfo( dir.filePath( "snapshot.json" ) ).permissions() &
                  ( QFile::ReadGroup | QFile::WriteGroup | QFile::ReadOther | QFile::WriteOther ),
              QFile::Permissions{} );
    // A repeated prepare after stopping must not replace the active snapshot.
    ops.active = []( bool& v, QString& )
    {
      v = false;
      return true;
    };
    QVERIFY( prepareSleep( store, ops, error ) );
    QVERIFY( resumeSleep( store, ops, error ) );
    QCOMPARE( calls.last(), "start" );
    QVERIFY( store.load( state, error ) );
    QVERIFY( !state );
    const auto count = calls.size();
    QVERIFY( resumeSleep( store, ops, error ) );
    QCOMPARE( calls.size(), count );
  }
  void stoppedDaemonStaysStopped()
  {
    QTemporaryDir dir;
    SleepStateStore store( dir.filePath( "snapshot.json" ), ::getuid() );
    QString error;
    int writes = 0;
    SleepOperations ops;
    ops.active = []( bool& v, QString& )
    {
      v = false;
      return true;
    };
    ops.capture = [&]( SleepState&, QString& )
    {
      ++writes;
      return true;
    };
    ops.stop = ops.start = [&]( QString& )
    {
      ++writes;
      return true;
    };
    QVERIFY( prepareSleep( store, ops, error ) );
    QVERIFY( resumeSleep( store, ops, error ) );
    QCOMPARE( writes, 0 );
  }
  void captureFailureDoesNotStopAndStartFailureRetainsState()
  {
    QTemporaryDir dir;
    SleepStateStore store( dir.filePath( "snapshot.json" ), ::getuid() );
    QString error;
    int stops = 0;
    SleepOperations ops;
    ops.active = []( bool& v, QString& )
    {
      v = true;
      return true;
    };
    ops.capture = []( SleepState&, QString& err )
    {
      err = "capture failure";
      return false;
    };
    ops.stop = [&]( QString& )
    {
      ++stops;
      return true;
    };
    ops.start = []( QString& err )
    {
      err = "start failure";
      return false;
    };
    QVERIFY( !prepareSleep( store, ops, error ) );
    QCOMPARE( stops, 0 );
    QVERIFY( store.save( SleepState{ true, "type_c", false }, error ) );
    QVERIFY( !resumeSleep( store, ops, error ) );
    std::optional< SleepState > saved;
    QVERIFY( store.load( saved, error ) );
    QVERIFY( saved );
    QCOMPARE( saved->manualMode, "type_c" );
    ops.start = []( QString& ) { return true; };
    QVERIFY( resumeSleep( store, ops, error ) );
  }
  void rejectsUntrustedMalformedAndSymlinkSnapshots()
  {
    QTemporaryDir dir;
    const auto path = dir.filePath( "snapshot.json" );
    SleepStateStore store( path, ::getuid() );
    QString error;
    QVERIFY( !store.save( SleepState{ true, "unknown", false }, error ) );
    QVERIFY( store.save( SleepState{ true, "ac", false }, error ) );
    std::optional< SleepState > state;
    SleepStateStore foreign( path, ::getuid() + 1 );
    QVERIFY( !foreign.load( state, error ) );
    QVERIFY( QFile::setPermissions( path, QFile::ReadOwner | QFile::WriteOwner | QFile::WriteOther ) );
    QVERIFY( !store.load( state, error ) );
    QVERIFY( QFile::setPermissions( path, QFile::ReadOwner | QFile::WriteOwner ) );
    QFile file( path );
    QVERIFY( file.open( QIODevice::WriteOnly | QIODevice::Truncate ) );
    file.write( "{}" );
    file.close();
    QVERIFY( !store.load( state, error ) );
    QVERIFY( QFile::remove( path ) );
    QVERIFY( QFile::link( dir.filePath( "target" ), path ) );
    QVERIFY( !store.load( state, error ) );
    QVERIFY( !store.save( SleepState{}, error ) );
    QVERIFY( QFile::remove( path ) );
    QVERIFY( QDir( dir.path() ).mkdir( "unsafe" ) );
    const auto unsafe = dir.filePath( "unsafe" );
    QVERIFY(
        QFile::setPermissions( unsafe, QFile::ReadOwner | QFile::WriteOwner | QFile::ExeOwner | QFile::WriteOther ) );
    QVERIFY( !SleepStateStore( unsafe + "/snapshot", ::getuid() ).save( SleepState{}, error ) );
  }
};
QTEST_GUILESS_MAIN( TestSleepState )
#include "test_sleep_state.moc"
