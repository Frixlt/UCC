# SPDX-License-Identifier: GPL-3.0-or-later
add_library(ucc-desktop-support STATIC
  SessionTelemetry.cpp SessionTelemetry.hpp
  ProcessTerminator.cpp ProcessTerminator.hpp
  SessionProcessesWidget.cpp SessionProcessesWidget.hpp)
target_include_directories(ucc-desktop-support PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(ucc-desktop-support PUBLIC ucc-dbus Qt6::Core Qt6::Widgets Qt6::DBus)

add_library(ucc-desktop-tray-support STATIC
  DesktopTray.cpp DesktopTray.hpp)
target_include_directories(ucc-desktop-tray-support PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(ucc-desktop-tray-support PUBLIC ucc-desktop-support ucc-native-tray)

if(BUILD_DESKTOP_TRAY)
  if(NOT UCC_PACKAGE_RUST_RUNTIME OR BUILD_TESTS)
    add_executable(ucc-desktop-tray main.cpp)
    target_link_libraries(ucc-desktop-tray PRIVATE ucc-desktop-tray-support)
  endif()
  if(NOT UCC_PACKAGE_RUST_RUNTIME)
    install(TARGETS ucc-desktop-tray RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
  endif()
  install(FILES ucc-desktop-tray.desktop DESTINATION ${UCC_AUTOSTART_DIR})
endif()
