#!/usr/bin/env bash
set -euo pipefail

TEST_BIN="${1:?usage: run_rust_interop_test.sh <test-binary> <source-dir>}"
SOURCE_DIR="${2:?usage: run_rust_interop_test.sh <test-binary> <source-dir>}"

export DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS"
export GIO_USE_VFS=local

DAEMON_BIN="${UCC_INTEROP_DAEMON_BIN:-$SOURCE_DIR/target/debug/examples/interop_daemon}"
if [[ ! -x "$DAEMON_BIN" ]]; then
  export PATH="$HOME/.cargo/bin:$PATH"
  if command -v cargo >/dev/null 2>&1; then
    RUSTUP_TOOLCHAIN="${RUSTUP_TOOLCHAIN:-stable}" \
      cargo build --manifest-path "$SOURCE_DIR/Cargo.toml" -p uccd --example interop_daemon --locked --offline
  fi
fi

if [[ ! -x "$DAEMON_BIN" ]]; then
  echo "Missing executable interop_daemon at: $DAEMON_BIN" >&2
  exit 1
fi

export UCC_INTEROP_DAEMON_BIN="$DAEMON_BIN"
exec "$TEST_BIN"
