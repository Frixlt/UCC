#!/usr/bin/env bash
set -euo pipefail
test_home=$(mktemp -d)
trap 'rm -rf "$test_home"' EXIT
export HOME="$test_home" \
  XDG_CONFIG_HOME="$test_home/.config" \
  XDG_DATA_HOME="$test_home/.local/share" \
  XDG_STATE_HOME="$test_home/.local/state" \
  XDG_CACHE_HOME="$test_home/.cache" \
  QT_QPA_PLATFORM=offscreen GIO_USE_VFS=local
export DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS"
"$@"
