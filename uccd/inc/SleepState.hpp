// SPDX-License-Identifier: GPL-3.0-or-later
#pragma once
#include <QDir>
#include <QFile>
#include <QFileInfo>
#include <QJsonDocument>
#include <QJsonObject>
#include <QSaveFile>
#include <cerrno>
#include <fcntl.h>
#include <functional>
#include <optional>
#include <sys/stat.h>
#include <unistd.h>

namespace ucc
{
struct SleepState
{
  bool wasActive = false;
  QString manualMode = "auto";
  bool coolerEnabled = false;
  static bool validMode( const QString& mode )
  {
    return mode == "auto" || mode == "ac" || mode == "type_c" || mode == "bat";
  }
};

// /run is volatile: these overrides survive sleep, never a reboot. The directory
// and file must be owned by root; the daemon never trusts a user's snapshot.
class SleepStateStore
{
public:
  explicit SleepStateStore( QString path = "/run/ucc/sleep-state.json", uid_t owner = 0 )
      : m_path( std::move( path ) ), m_owner( owner )
  {
  }
  bool load( std::optional< SleepState >& state, QString& error ) const
  {
    state.reset();
    struct stat info{};
    if( ::lstat( QFile::encodeName( m_path ).constData(), &info ) != 0 )
    {
      if( errno == ENOENT )
        return true;
      error = "Cannot inspect sleep snapshot";
      return false;
    }
    if( !directory( false, error ) )
      return false;
    const int fd = ::open( QFile::encodeName( m_path ).constData(), O_RDONLY | O_CLOEXEC | O_NOFOLLOW | O_NONBLOCK );
    if( fd < 0 )
    {
      error = "Cannot open sleep snapshot";
      return false;
    }
    QFile file;
    if( !file.open( fd, QIODevice::ReadOnly, QFileDevice::AutoCloseHandle ) )
    {
      ::close( fd );
      error = "Cannot read sleep snapshot";
      return false;
    }
    if( ::fstat( fd, &info ) != 0 || !S_ISREG( info.st_mode ) || info.st_uid != m_owner ||
        ( info.st_mode & 077 ) != 0 || info.st_size > 4096 )
    {
      error = "Unsafe sleep snapshot ownership, permissions or size";
      return false;
    }
    const auto doc = QJsonDocument::fromJson( file.read( 4097 ) );
    const auto json = doc.object();
    if( !doc.isObject() || json["version"] != 1 || !json["wasActive"].isBool() || !json["coolerEnabled"].isBool() ||
        !SleepState::validMode( json["manualMode"].toString() ) )
    {
      error = "Invalid sleep snapshot";
      return false;
    }
    state = SleepState{ json["wasActive"].toBool(), json["manualMode"].toString(), json["coolerEnabled"].toBool() };
    return true;
  }
  bool save( const SleepState& state, QString& error ) const
  {
    if( !SleepState::validMode( state.manualMode ) )
    {
      error = "Invalid manual power mode";
      return false;
    }
    if( !directory( true, error ) )
      return false;
    std::optional< SleepState > existing;
    if( !load( existing, error ) )
      return false;
    QSaveFile file( m_path );
    file.setDirectWriteFallback( false );
    if( !file.open( QIODevice::WriteOnly ) || !file.setPermissions( QFile::ReadOwner | QFile::WriteOwner ) )
    {
      error = "Cannot create private sleep snapshot";
      return false;
    }
    const auto data = QJsonDocument( QJsonObject{ { "version", 1 },
                                                  { "wasActive", state.wasActive },
                                                  { "manualMode", state.manualMode },
                                                  { "coolerEnabled", state.coolerEnabled } } )
                          .toJson( QJsonDocument::Compact );
    if( file.write( data ) != data.size() || !file.commit() )
    {
      error = "Cannot save sleep snapshot";
      return false;
    }
    return true;
  }
  bool clear( QString& error ) const
  {
    std::optional< SleepState > state;
    if( !load( state, error ) )
      return false;
    if( !state || QFile::remove( m_path ) )
      return true;
    error = "Cannot remove sleep snapshot";
    return false;
  }

private:
  bool directory( bool create, QString& error ) const
  {
    const auto path = QFileInfo( m_path ).absolutePath();
    const auto name = QFile::encodeName( path );
    struct stat info{};
    if( create && ::mkdir( name.constData(), 0700 ) != 0 && errno != EEXIST )
    {
      error = "Cannot create sleep snapshot directory";
      return false;
    }
    if( ::lstat( name.constData(), &info ) != 0 || !S_ISDIR( info.st_mode ) || info.st_uid != m_owner ||
        ( info.st_mode & 022 ) != 0 )
    {
      error = "Unsafe sleep snapshot directory";
      return false;
    }
    return true;
  }
  QString m_path;
  uid_t m_owner;
};

// Explicit operations make the lifecycle testable without touching systemd,
// D-Bus, hardware or /run. A failed start retains the state for a later retry.
struct SleepOperations
{
  std::function< bool( bool&, QString& ) > active;
  std::function< bool( SleepState&, QString& ) > capture;
  std::function< bool( QString& ) > stop, start;
};
inline bool prepareSleep( const SleepStateStore& store, const SleepOperations& ops, QString& error )
{
  std::optional< SleepState > previous;
  if( !store.load( previous, error ) )
    return false;
  bool active = false;
  if( !ops.active( active, error ) )
    return false;
  if( !active )
  {
    return previous ? true : store.save( SleepState{}, error );
  }
  SleepState state;
  state.wasActive = true;
  if( !ops.capture( state, error ) || !store.save( state, error ) )
    return false;
  if( ops.stop( error ) )
    return true;
  // If stopping failed, keep the snapshot: resume can still recover the daemon.
  return false;
}
inline bool resumeSleep( const SleepStateStore& store, const SleepOperations& ops, QString& error )
{
  std::optional< SleepState > state;
  if( !store.load( state, error ) )
    return false;
  if( !state )
    return true;
  if( state->wasActive && !ops.start( error ) )
    return false;
  return store.clear( error );
}
} // namespace ucc
