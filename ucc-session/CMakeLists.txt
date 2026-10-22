# SPDX-License-Identifier: GPL-3.0-or-later
add_executable(ucc-session-agent main.cpp)
target_link_libraries(ucc-session-agent PRIVATE ucc-dbus Qt6::Core Qt6::DBus)
install(TARGETS ucc-session-agent RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
install(FILES ucc-session-agent.desktop DESTINATION ${CMAKE_INSTALL_FULL_SYSCONFDIR}/xdg/autostart)
