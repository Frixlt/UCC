/*
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program.  If not, see <https://www.gnu.org/licenses/>.
 */

#include "DetectLayout.hpp"
#include <QProcess>
#include <QDebug>
#include <QCoreApplication>
#include <QElapsedTimer>
#include <QStandardPaths>
#include <algorithm>
#include <optional>

namespace ucc
{

// Helper function to normalize layout names to our supported formats
QString normalizeLayout(const QString &layout)
{
  QString lower = layout.toLower();

  if (lower == "tr" || lower == "turkish" || lower == "türkçe" || lower.contains("turk")) {
    return "tr";
  }
  if (lower == "fr" || lower == "french" || lower == "français" || lower.contains("franc")) {
    return "fr";
  }
  if (lower == "es" || lower == "spanish" || lower == "español" || lower.contains("span")) {
    return "es";
  }
  if (lower == "it" || lower == "italian" || lower == "italiano" || lower.contains("ital")) {
    return "it";
  }
  if (lower == "ar" || lower == "arabic" || lower == "العربية" || lower.contains("arab")) {
    return "ar";
  }
  if (lower == "de" || lower == "deutsch" || lower == "german" || lower.contains("deut")) {
    return "de";
  }
  if (lower == "us" || lower == "en" || lower == "english" || lower.contains("engl")) {
    return "us";
  }

  return lower;
}

QString detectKeyboardLayout()
{
  // Fast env-var check avoids spawning any subprocess when XKB_DEFAULT_LAYOUT is provided.
  if ( QString xkbLayout = qgetenv( "XKB_DEFAULT_LAYOUT" ); !xkbLayout.isEmpty() )
  {
    QString normalized = normalizeLayout( xkbLayout.split( ',' ).first() );
    if ( !normalized.isEmpty() )
    {
      qDebug() << "Detected layout from XKB_DEFAULT_LAYOUT:" << normalized;
      return normalized;
    }
  }

  QElapsedTimer budget;
  budget.start();
  static constexpr qint64 kTotalBudgetMs = 500;
  static constexpr qint64 kPerProcessMs = 250;

  auto runBounded = [&]( const QString &program, const QStringList &args ) -> std::optional< QString > {
    const qint64 remaining = kTotalBudgetMs - budget.elapsed();
    if ( remaining <= 0 )
      return std::nullopt;
    const QString exe = QStandardPaths::findExecutable( program );
    if ( exe.isEmpty() )
      return std::nullopt;

    QProcess process;
    process.start( exe, args );
    const int waitMs = static_cast< int >( std::min< qint64 >( kPerProcessMs, remaining ) );
    if ( !process.waitForStarted( waitMs ) )
    {
      process.kill();
      process.waitForFinished( 50 );
      return std::nullopt;
    }
    const qint64 afterStartRemaining = kTotalBudgetMs - budget.elapsed();
    if ( afterStartRemaining <= 0 ||
         !process.waitForFinished( static_cast< int >( std::min< qint64 >( kPerProcessMs, afterStartRemaining ) ) ) )
    {
      process.kill();
      process.waitForFinished( 50 );
      return std::nullopt;
    }
    return QString::fromLocal8Bit( process.readAllStandardOutput() );
  };

  const QString desktop = qgetenv( "XDG_CURRENT_DESKTOP" ).toLower();
  const QString session = qgetenv( "DESKTOP_SESSION" ).toLower();
  const bool isKDE = desktop.contains( "kde" ) || session.contains( "kde" ) || !qgetenv( "KDE_SESSION_UID" ).isEmpty();
  const bool isGNOME = desktop.contains( "gnome" ) || session.contains( "gnome" );
  const bool isWayland = !qgetenv( "WAYLAND_DISPLAY" ).isEmpty();

  if ( isKDE )
  {
    if ( auto idxOut = runBounded(
           QStringLiteral( "dbus-send" ),
           { QStringLiteral( "--session" ), QStringLiteral( "--dest=org.kde.keyboard" ),
             QStringLiteral( "--print-reply" ), QStringLiteral( "/Layouts" ),
             QStringLiteral( "org.kde.KeyboardLayouts.getLayout" ) } ) )
    {
      uint currentIndex = 0;
      for ( const QString &line : idxOut->trimmed().split( '\n' ) )
      {
        if ( line.contains( "uint32" ) )
        {
          currentIndex = line.split( ' ' ).last().toUInt();
          break;
        }
      }
      if ( auto listOut = runBounded(
             QStringLiteral( "dbus-send" ),
             { QStringLiteral( "--session" ), QStringLiteral( "--dest=org.kde.keyboard" ),
               QStringLiteral( "--print-reply" ), QStringLiteral( "/Layouts" ),
               QStringLiteral( "org.kde.KeyboardLayouts.getLayoutsList" ) } ) )
      {
        QStringList layouts;
        int count = 0;
        for ( const QString &line : listOut->trimmed().split( '\n' ) )
        {
          if ( line.contains( "string \"" ) )
          {
            QString value = line.split( '"' ).at( 1 );
            if ( count % 3 == 0 )
              layouts.append( value );
            count++;
          }
        }
        if ( currentIndex < static_cast< uint >( layouts.size() ) )
          return normalizeLayout( layouts.at( static_cast< int >( currentIndex ) ) );
      }
    }
  }
  else if ( isGNOME )
  {
    if ( auto output = runBounded(
           QStringLiteral( "gsettings" ),
           { QStringLiteral( "get" ), QStringLiteral( "org.gnome.desktop.input-sources" ),
             QStringLiteral( "sources" ) } ) )
    {
      const QStringList supportedLayouts = { "tr", "fr", "es", "it", "ar", "de", "us", "en" };
      for ( const QString &layout : supportedLayouts )
      {
        if ( output->contains( layout ) ||
             ( layout == "tr" && ( output->contains( "turkish" ) || output->contains( "türkçe" ) ) ) ||
             ( layout == "fr" && ( output->contains( "french" ) || output->contains( "français" ) ) ) ||
             ( layout == "es" && ( output->contains( "spanish" ) || output->contains( "español" ) ) ) ||
             ( layout == "it" && ( output->contains( "italian" ) || output->contains( "italiano" ) ) ) ||
             ( layout == "ar" && ( output->contains( "arabic" ) || output->contains( "العربية" ) ) ) ||
             ( layout == "de" && ( output->contains( "deutsch" ) || output->contains( "german" ) ) ) ||
             ( layout == "us" && ( output->contains( "english" ) || output->contains( "en" ) ) ) )
        {
          return layout;
        }
      }
    }
  }

  if ( auto output = runBounded( QStringLiteral( "localectl" ), {} ) )
  {
    for ( const QString &line : output->split( '\n' ) )
    {
      if ( line.contains( "X11 Layout:" ) || line.contains( "VC Keymap:" ) )
      {
        QString layout = line.split( ':' ).value( 1 ).trimmed();
        QString normalized = normalizeLayout( layout.split( ',' ).first() );
        if ( !normalized.isEmpty() )
          return normalized;
      }
    }
  }

  if ( auto output = runBounded( QStringLiteral( "setxkbmap" ), { QStringLiteral( "-query" ) } ) )
  {
    for ( const QString &line : output->split( '\n' ) )
    {
      if ( line.startsWith( "layout:" ) )
      {
        QString layout = line.split( ':' ).value( 1 ).trimmed();
        QString normalized = normalizeLayout( layout );
        if ( !normalized.isEmpty() && ( !isWayland || normalized != "us" ) )
          return normalized;
      }
    }
  }

  if ( isWayland )
  {
    if ( auto output = runBounded(
           QStringLiteral( "swaymsg" ),
           { QStringLiteral( "-t" ), QStringLiteral( "get_inputs" ) } ) )
    {
      const QStringList supportedLayouts = { "tr", "fr", "es", "it", "ar", "de", "us", "en" };
      for ( const QString &layout : supportedLayouts )
      {
        if ( output->contains( "\"" + layout + "\"" ) || output->contains( "'" + layout + "'" ) ||
             ( layout == "tr" && ( output->contains( "\"turkish\"" ) || output->contains( "'turkish'" ) || output->contains( "\"türkçe\"" ) || output->contains( "'türkçe'" ) ) ) ||
             ( layout == "fr" && ( output->contains( "\"french\"" ) || output->contains( "'french'" ) || output->contains( "\"français\"" ) || output->contains( "'français'" ) ) ) ||
             ( layout == "es" && ( output->contains( "\"spanish\"" ) || output->contains( "'spanish'" ) || output->contains( "\"español\"" ) || output->contains( "'español'" ) ) ) ||
             ( layout == "it" && ( output->contains( "\"italian\"" ) || output->contains( "'italian'" ) || output->contains( "\"italiano\"" ) || output->contains( "'italiano'" ) ) ) ||
             ( layout == "ar" && ( output->contains( "\"arabic\"" ) || output->contains( "'arabic'" ) || output->contains( "\"العربية\"" ) || output->contains( "'العربية'" ) ) ) ||
             ( layout == "de" && ( output->contains( "\"deutsch\"" ) || output->contains( "'deutsch'" ) || output->contains( "\"german\"" ) || output->contains( "'german'" ) ) ) ||
             ( layout == "us" && ( output->contains( "\"english\"" ) || output->contains( "'english'" ) || output->contains( "\"en\"" ) || output->contains( "'en'" ) ) ) )
        {
          return layout;
        }
      }
    }
  }

  if ( QString lang = qgetenv( "LANG" ); !lang.isEmpty() )
  {
    if ( QString normalizedLang = normalizeLayout( lang ); !normalizedLang.isEmpty() && normalizedLang != "us" )
      return normalizedLang;
  }

  return "us";
}

}
