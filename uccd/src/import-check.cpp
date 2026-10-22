// SPDX-License-Identifier: GPL-3.0-or-later
// Offline preflight: never opens hardware, D-Bus or installed settings.
#include "LightbarController.hpp"
#include "PowerDeviceController.hpp"
#include "ProfileManager.hpp"
#include "SettingsManager.hpp"
#include <QCoreApplication>
#include <QFile>
#include <iostream>
using Json = nlohmann::json;
namespace
{
void require( bool ok, const std::string& message )
{
  if( !ok )
    throw std::runtime_error( message );
}
Json read( const QString& path )
{
  QFile file( path );
  require( file.open( QIODevice::ReadOnly ), "Cannot open input" );
  require( file.size() <= 16 * 1024 * 1024, "Input exceeds size limit" );
  return Json::parse( file.readAll().toStdString() );
}
} // namespace
int main( int argc, char** argv )
{
  QCoreApplication app( argc, argv );
  if( argc != 3 )
  {
    std::cerr << "Usage: ucc-import-check SETTINGS CAPABILITIES\n";
    return 2;
  }
  try
  {
    const auto root = read( app.arguments()[1] ), caps = read( app.arguments()[2] );
    require( SettingsManager().parseSettingsJSON( root.dump() ).has_value(), "UCC settings parser rejected import" );
    std::string error;
    Json normalized;
    require( PowerDeviceController::validate( root.at( "powerDevices" ), error ), error );
    require( LightbarController::normalize( root.at( "lightbar" ), normalized, error ), error );
    require( root.at( "keyboardLayout" ) == "frixit-mechrevo", "Personal keyboard layout is missing" );
    const int maximum = caps.at( "maxBrightness" ).get< int >();
    const int zones = caps.at( "zones" ).get< int >();
    require( maximum > 0 && zones > 0 && zones <= 1024, "Invalid keyboard capabilities" );
    const auto states = root.at( "keyboardBacklightStates" );
    require( states.is_array() && states.size() == static_cast< size_t >( zones ),
             "Keyboard zone count differs from hardware" );
    for( const auto& state : states )
    {
      for( const auto* channel : { "red", "green", "blue", "brightness" } )
      {
        require( state.at( channel ).is_number_integer(), "Invalid keyboard channel" );
        const int value = state.at( channel ).get< int >();
        require( value >= 0 && value <= ( std::string( channel ) == "brightness" ? maximum : 255 ),
                 "Keyboard value out of range" );
      }
    }
    for( const auto* mode : { "ac", "type_c", "bat" } )
    {
      const auto id = root.at( "stateMap" ).at( std::string( "power_" ) + mode ).get< std::string >();
      const auto& source = root.at( "profiles" ).at( id );
      require( source.is_object(), "Import profiles must be JSON objects" );
      const auto parsed = ProfileManager::parseProfileJSON( source.dump() );
      const auto roundtrip = Json::parse( ProfileManager::profileToJSON( parsed ) );
      require( parsed.id == id, "Profile identity changed during UCC parsing" );
      require( parsed.keyboard.keyboardProfileId == "frixit-keyboard", "Personal keyboard profile is missing" );
      require( root.at( "customKeyboardProfiles" ).at( "frixit-keyboard" ).at( "json" ).at( "states" ) == states,
               "Keyboard profile disagrees with startup colors" );
      for( const auto* key : { "onlineCores", "scalingMinFrequency", "scalingMaxFrequency", "governor",
                               "energyPerformancePreference", "noTurbo" } )
        if( source.at( "cpu" ).contains( key ) )
          require( roundtrip.at( "cpu" ).at( key ) == source.at( "cpu" ).at( key ),
                   std::string( "CPU round-trip changed " ) + key );
      for( const auto* key : { "odmPowerLimits", "nvidiaCTGPOffset" } )
        if( source.contains( key ) )
          require( roundtrip.at( key ) == source.at( key ), std::string( "Power limit round-trip changed " ) + key );
      for( const auto* key : { "tableCPU", "tableGPU", "tableWaterCoolerFan", "tablePump" } )
      {
        if( !source.at( "fan" ).contains( key ) )
          continue;
        const auto& table = source.at( "fan" ).at( key );
        require( roundtrip.at( "fan" ).at( key ) == table, std::string( "Fan round-trip changed " ) + key );
        int previous = -1;
        for( const auto& point : table )
        {
          const int temp = point.at( "temp" ).get< int >(), speed = point.at( "speed" ).get< int >();
          require( temp >= 0 && temp <= 100 && temp > previous && speed >= 0 && speed <= 100,
                   "Invalid fan curve point" );
          previous = temp;
        }
      }
    }
    std::cout << Json( { { "ok", true },
                         { "profiles", 3 },
                         { "keyboardZones", zones },
                         { "devices", root.at( "powerDevices" ).at( "devices" ).size() } } )
                     .dump()
              << '\n';
    return 0;
  }
  catch( const std::exception& e )
  {
    std::cerr << "Import rejected: " << e.what() << '\n';
    return 1;
  }
}
