# SPDX-License-Identifier: GPL-3.0-or-later
add_library(ucc-session-support STATIC RefreshAdvisor.cpp RefreshAdvisor.hpp)
target_include_directories(ucc-session-support PUBLIC ${CMAKE_CURRENT_SOURCE_DIR})
target_link_libraries(ucc-session-support PUBLIC ucc-dbus Qt6::Core Qt6::DBus)
add_executable(ucc-session-agent main.cpp)
target_link_libraries(ucc-session-agent PRIVATE ucc-session-support ucc-dbus Qt6::Core Qt6::DBus)
install(TARGETS ucc-session-agent RUNTIME DESTINATION ${CMAKE_INSTALL_BINDIR})
install(FILES ucc-session-agent.desktop DESTINATION ${CMAKE_INSTALL_FULL_SYSCONFDIR}/xdg/autostart)
