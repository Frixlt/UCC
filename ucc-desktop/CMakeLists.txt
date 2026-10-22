# SPDX-License-Identifier: GPL-3.0-or-later
add_library(ucc-desktop-support STATIC
  SessionTelemetry.cpp SessionTelemetry.hpp
  ProcessTerminator.cpp ProcessTerminator.hpp
  SessionProcessesWidget.cpp SessionProcessesWidget.hpp
  DesktopTray.cpp DesktopTray.hpp)
target_include_directories(ucc-desktop-support PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(ucc-desktop-support PUBLIC ucc-dbus Qt6::Core Qt6::Widgets Qt6::DBus ucc-native-tray)
if(BUILD_DESKTOP_TRAY)
  add_executable(ucc-desktop-tray main.cpp)
  target_link_libraries(ucc-desktop-tray PRIVATE ucc-desktop-support)
  install(TARGETS ucc-desktop-tray RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
  install(FILES ucc-desktop-tray.desktop DESTINATION ${CMAKE_INSTALL_FULL_SYSCONFDIR}/xdg/autostart)
endif()
