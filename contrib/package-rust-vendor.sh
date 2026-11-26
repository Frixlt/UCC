#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
set -euo pipefail

# Run on a fresh source export before creating a distribution archive.
cd "${1:?expected a source export directory}"
mkdir -p .cargo
config=$(mktemp)
trap 'rm -f "$config"' EXIT
cargo vendor --locked --versioned-dirs vendor > "$config"
printf '\n' >> .cargo/config.toml
cat "$config" >> .cargo/config.toml
