#!/usr/bin/env bash
set -euo pipefail

TEST_BIN="${1:?usage: run_rust_interop_test.sh <test-binary> <source-dir>}"
SOURCE_DIR="${2:?usage: run_rust_interop_test.sh <test-binary> <source-dir>}"
shift 2

ORIG_HOME="${HOME:-}"
if [[ -z "${CARGO_HOME:-}" && -n "$ORIG_HOME" ]]; then
  export CARGO_HOME="$ORIG_HOME/.cargo"
fi
if [[ -z "${RUSTUP_HOME:-}" && -n "$ORIG_HOME" ]]; then
  export RUSTUP_HOME="$ORIG_HOME/.rustup"
fi
if [[ -n "$ORIG_HOME" && -d "$ORIG_HOME/.cargo/bin" ]]; then
  export PATH="$ORIG_HOME/.cargo/bin:$PATH"
fi

test_home=$(mktemp -d)
trap 'rm -rf "$test_home"' EXIT
export HOME="$test_home" \
  XDG_CONFIG_HOME="$test_home/.config" \
  XDG_DATA_HOME="$test_home/.local/share" \
  XDG_STATE_HOME="$test_home/.local/state" \
  XDG_CACHE_HOME="$test_home/.cache"
export DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS"
export GIO_USE_VFS=local

TARGET_DIR="${CARGO_TARGET_DIR:-$SOURCE_DIR/target}"
DAEMON_BIN="${UCC_INTEROP_DAEMON_BIN:-$TARGET_DIR/debug/examples/interop_daemon}"
if [[ ! -x "$DAEMON_BIN" ]]; then
  if command -v cargo >/dev/null 2>&1; then
    if ! cargo build --manifest-path "$SOURCE_DIR/Cargo.toml" -p uccd --example interop_daemon --locked --offline; then
      echo "Offline cargo build of interop_daemon failed (possibly an empty CARGO_HOME cache)." >&2
      echo "Provide a prebuilt binary via UCC_INTEROP_DAEMON_BIN=/path/to/interop_daemon or pre-populate Cargo cache." >&2
      exit 1
    fi
  else
    echo "Cargo not found and no prebuilt UCC_INTEROP_DAEMON_BIN provided." >&2
    exit 1
  fi
fi

if [[ ! -x "$DAEMON_BIN" ]]; then
  echo "Missing executable interop_daemon at: $DAEMON_BIN" >&2
  exit 1
fi

export UCC_INTEROP_DAEMON_BIN="$DAEMON_BIN"
status=0
"$TEST_BIN" "$@" || status=$?
exit "$status"
