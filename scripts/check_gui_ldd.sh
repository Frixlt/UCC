#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Verify M12.7 / ADR-01 dynamic linkage invariants for ucc-gui:
#   - no unresolved libraries ("not found")
#   - no KDE Frameworks 6 (KF6 / libKF6*)
#   - no Qt6 QML / Quick (Qt6Qml, Qt6Quick)
#   - no Qt6 Bluetooth (Qt6Bluetooth)
#   - no shared libucc-dbus.so (must be linked statically per ADR-01)

set -euo pipefail

usage() {
  cat <<'EOF'
Usage: check_gui_ldd.sh <path-to-ucc-gui>

Checks `ldd` output for the ucc-gui binary against M12.7 / ADR-01 linkage
invariants (no `not found`, `KF6`, `Qt6Qml`, `Qt6Quick`, `Qt6Bluetooth`, or
shared `libucc-dbus.so`).

Testing / mock environment variables:
  UCC_LDD_BIN          Override `ldd` executable (default: ldd)
  UCC_LDD_OUTPUT       Provide raw `ldd` output directly (skips running ldd)
  UCC_LDD_OUTPUT_FILE  Read raw `ldd` output from a file (skips running ldd)
EOF
}

bin=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -h|--help)
      usage
      exit 0
      ;;
    --)
      shift
      if [ "$#" -gt 0 ]; then
        if [ -n "$bin" ]; then
          echo "ERROR: unexpected extra argument: $1" >&2
          usage >&2
          exit 2
        fi
        bin="$1"
        shift
      fi
      if [ "$#" -gt 0 ]; then
        echo "ERROR: unexpected extra arguments: $*" >&2
        usage >&2
        exit 2
      fi
      ;;
    -*)
      echo "ERROR: unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
    *)
      if [ -n "$bin" ]; then
        echo "ERROR: unexpected extra argument: $1" >&2
        usage >&2
        exit 2
      fi
      bin="$1"
      shift
      ;;
  esac
done

ldd_out=""
target_label="${bin:-<mock>}"

if [ -n "${UCC_LDD_OUTPUT+x}" ]; then
  ldd_out="$UCC_LDD_OUTPUT"
elif [ -n "${UCC_LDD_OUTPUT_FILE:-}" ]; then
  if [ ! -f "$UCC_LDD_OUTPUT_FILE" ]; then
    echo "ERROR: UCC_LDD_OUTPUT_FILE does not exist: $UCC_LDD_OUTPUT_FILE" >&2
    exit 2
  fi
  ldd_out="$(cat -- "$UCC_LDD_OUTPUT_FILE")"
else
  if [ -z "$bin" ]; then
    echo "ERROR: missing path to ucc-gui binary" >&2
    usage >&2
    exit 2
  fi
  if [ ! -f "$bin" ]; then
    echo "ERROR: binary not found: $bin" >&2
    exit 2
  fi
  if [ ! -r "$bin" ]; then
    echo "ERROR: binary is not readable: $bin" >&2
    exit 2
  fi
  ldd_cmd="${UCC_LDD_BIN:-ldd}"
  if ! command -v "$ldd_cmd" >/dev/null 2>&1; then
    echo "ERROR: ldd command not found: $ldd_cmd" >&2
    exit 2
  fi
  ldd_out="$("$ldd_cmd" -- "$bin")"
fi

if [ -z "$(printf '%s' "$ldd_out" | tr -d '[:space:]')" ]; then
  echo "ERROR: empty ldd output for $target_label" >&2
  exit 1
fi

violations=0

check_forbidden() {
  local label="$1"
  local pattern="$2"
  local matches=""
  if matches="$(printf '%s\n' "$ldd_out" | grep -E -- "$pattern" || true)" && [ -n "$matches" ]; then
    echo "FAIL [$label]: forbidden ldd dependency detected in $target_label:" >&2
    printf '%s\n' "$matches" | sed 's/^/  /' >&2
    violations=$((violations + 1))
  fi
}

check_forbidden "not found" '(^|[[:space:]])not found([[:space:]]|$)'
check_forbidden "KF6" '(lib)?KF6[A-Za-z0-9_.-]*'
check_forbidden "Qt6Qml" 'Qt6Qml'
check_forbidden "Qt6Quick" 'Qt6Quick'
check_forbidden "Qt6Bluetooth" 'Qt6Bluetooth'
check_forbidden "libucc-dbus.so" 'libucc-dbus(-preview)?\.so'

if [ "$violations" -gt 0 ]; then
  echo "ERROR: $violations ldd invariant violation(s) found for $target_label (M12.7 / ADR-01)" >&2
  exit 1
fi

echo "OK: ucc-gui ldd invariants satisfied ($target_label)"
