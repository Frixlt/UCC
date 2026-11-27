# Building, Packaging, and Installation (`ucc-rusted`)

[Testing and Verification](TESTING.md) · [Migration and Recovery](MIGRATION_AND_RECOVERY.md)

This document covers building the `ucc-rusted` Rust workspace, the standalone `Qt6` client `ucc-gui`, `UCC_READ_ONLY_PREVIEW` builds, `cargo xtask` automation, and packaging for Arch Linux, Debian/Ubuntu, Fedora/RPM, and Nix/NixOS.

---

## 1. Build Prerequisites

### Base Dependencies (Rust Runtime + Standalone Qt6 GUI)

| Component | Minimum Version / Package | Purpose |
| --- | --- | --- |
| Rust / Cargo | MSRV `1.89` (`Cargo.toml`), pinned toolchain `1.99.1` (`rust-toolchain.toml` with `rustfmt`, `clippy`) | Building all 10 workspace crates (`edition = "2021"`) |
| C++ Compiler | C++20 (`g++ >= 9` or compatible Clang) | Building static `libucc-dbus`, `ucc-desktop-support`, and `ucc-gui` |
| CMake / Ninja | `cmake >= 3.20`, `ninja` | Configuring and building the Qt6 client |
| Build Tools | `git`, `pkg-config` (`pkgconf`) | Resolving short git commit hash and system libraries |
| Qt6 | `Qt6::Core`, `Qt6::DBus`, `Qt6::Gui`, `Qt6::Widgets`, `Qt6::Charts` | `ucc-gui` interface and telemetry charts |
| System Libraries | `libxcb`, `systemd-libs`, `dbus`, `polkit`, `hicolor-icon-theme`, `nlohmann-json` | Optional X11 backdrop blur (`UCC_XCB`), D-Bus, Polkit, systemd integration |
| Optional (Runtime) | `tuxedo-drivers`, `bluez`, `kscreen`, `qt6-wayland` | `/dev/tuxedo_io` interface, BLE water cooler, KDE refresh-rate offers, native Wayland |

### GUI Dependency Boundary (`M12.7` / `ADR-01`)

Release builds of the standalone Qt6 GUI with the Rust runtime (`-DUCC_PACKAGE_RUST_RUNTIME=ON -DBUILD_TRAY=OFF`) **do not require** and **must not link**:

- **KF6 / Plasma / ECM**: `extra-cmake-modules`, `KF6Config`, `KF6CoreAddons`, `KF6WindowSystem`, `KF6StatusNotifierItem`, `Plasma`;
- **Qt6 QML / Quick**: `Qt6Qml`, `Qt6Quick` (`qt6-declarative`);
- **Qt6 Bluetooth**: `Qt6Bluetooth` (`qt6-connectivity`);
- **Shared `libucc-dbus.so`**: `libucc-dbus` (`libucc-dbus/CMakeLists.txt`) is built as a `STATIC` archive and linked statically into `ucc-gui`.

`KF6` and `Plasma` are requested only when `-DBUILD_TRAY=ON` (retired Plasma applet, `ADR-03`) or `-DUCC_PACKAGE_RUST_RUNTIME=OFF`. `Qt6Bluetooth` and `libudev` are required only when building legacy C++ `uccd` or C++ unit tests (`-DBUILD_TESTS=ON`).

### Distribution Package Dependencies

**Arch Linux / CachyOS** (matches `PKGBUILD`):

```bash
# Minimal set for release Rust runtime + standalone Qt6 GUI
sudo pacman -S --needed base-devel rust cmake ninja git pkgconf \
  qt6-base qt6-charts qt6-wayland systemd-libs \
  dbus polkit hicolor-icon-theme libxcb nlohmann-json

# Additional packages for CTest (C++ / Python / GNOME tests)
sudo pacman -S --needed qt6-connectivity python gjs nodejs
```

**Debian / Ubuntu** (matches `debian/control`):

```bash
sudo apt-get update
sudo apt-get install -y --no-install-recommends \
  debhelper cargo rustc cmake ninja-build g++ git pkgconf \
  nlohmann-json3-dev qt6-base-dev qt6-charts-dev qt6-tools-dev \
  libsystemd-dev libxrandr-dev libudev-dev libxcb1-dev dbus polkitd
```

**Fedora / RHEL** (matches `ucc.spec`):

```bash
sudo dnf install -y \
  cmake ninja-build cargo rust gcc-c++ git pkgconf-pkg-config \
  nlohmann-json-devel qt6-qtbase-devel qt6-qtcharts-devel \
  systemd-devel libXrandr-devel libgudev-devel libxcb-devel dbus polkit
```

---

## 2. Building the Rust Workspace and Offline Tarball

The `Cargo.toml` workspace contains 10 crates:

- `crates/ucc-core` — domain models, validation, DTOs;
- `crates/ucc-config` — loading, atomic writes, `.bak` recovery, offline TCC/legacy import;
- `crates/ucc-hardware` — `/dev/tuxedo_io`, CPU/GPU sysfs, NVML (`libloading`), BlueZ GATT;
- `crates/ucc-dbus` — `com.uniwill.uccd` / `de.uniwill.uccd` D-Bus contracts, proxies, Polkit;
- `crates/uccd` — `uccd` daemon and root helpers `ucc-hardware`, `ucc-sleep`, `ucc-import-check`;
- `crates/ucc-cli` — `ucc-cli` command-line tool;
- `crates/ucc-desktop` — standalone StatusNotifierItem tray `ucc-desktop-tray`;
- `crates/ucc-session-agent` — user-session agent `ucc-session-agent`;
- `crates/ucc-lighting` — per-key keyboard RGB and Lightbar controllers;
- `crates/xtask` — build, packaging, release, and verification automation.

### Local Rust Build

With a populated Cargo cache (`cargo fetch --locked`) or vendored sources, prefer an out-of-tree target directory during development:

```bash
# Out-of-tree development / CI build
CARGO_TARGET_DIR=/tmp/ucc-cargo-target \
  cargo build --workspace --release --locked --offline

# Release runtime binaries only (matches Makefile / PKGBUILD / ucc.spec)
cargo build --workspace --bins --release --locked --offline --target-dir target
```

Compiled executables in `<target-dir>/release/`: `uccd`, `ucc-cli`, `ucc-desktop-tray`, `ucc-session-agent`, `ucc-hardware`, `ucc-sleep`, `ucc-import-check`, `xtask`.

### Preparing a Vendored Offline Source Archive

For reproducible offline package builds (`make deb`, `make rpm`, air-gapped builders), run:

```bash
make dist
```

`make dist` performs:
1. Exports `HEAD` via `git archive --format=tar --prefix=ucc-<VERSION>/`.
2. Writes the short commit SHA into `.release-commit` (read by `xtask` when `.git` is absent).
3. Runs `bash contrib/package-rust-vendor.sh <export-dir>`:
   - invokes `cargo vendor --locked --versioned-dirs vendor`;
   - appends `[source.crates-io]` and `[source.vendored-sources]` to `.cargo/config.toml` inside the export directory.
4. Archives `dist/ucc-<VERSION>-<GITHASH>.tar.gz` and generates `dist/ucc-<VERSION>-<GITHASH>.tar.gz.sha256`.

Direct invocation on an already extracted source tree:

```bash
bash contrib/package-rust-vendor.sh /path/to/exported/ucc-0.0.1
```

---

## 3. Building Standalone Qt6 `ucc-gui` and Read-Only Preview

### Normal Build (`-DUCC_READ_ONLY_PREVIEW=OFF`)

Out-of-tree standalone release build for the Rust runtime:

```bash
cmake -S . -B /tmp/ucc-build-normal -G Ninja \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr \
  -DCMAKE_INSTALL_LIBDIR=lib \
  -DBUILD_GUI=ON \
  -DBUILD_TRAY=OFF \
  -DBUILD_CLI=OFF \
  -DUCC_PACKAGE_RUST_RUNTIME=ON \
  -DUCC_READ_ONLY_PREVIEW=OFF
cmake --build /tmp/ucc-build-normal --parallel 2

# Verify zero unresolved libraries and zero forbidden links (M12.7 / ADR-01)
bash scripts/check_gui_ldd.sh /tmp/ucc-build-normal/bin/ucc-gui
```

With `-DUCC_PACKAGE_RUST_RUNTIME=ON`:
- CMake builds static `libucc-dbus`, static `ucc-desktop-support`, and `ucc-gui` (with `UCC_PACKAGE_RUST_RUNTIME=1`);
- `uccd/CMakeLists.txt` installs only systemd, D-Bus, and Polkit files and skips C++ `uccd` (when `-DBUILD_TESTS=OFF`);
- `ucc-desktop/CMakeLists.txt` and `ucc-session/CMakeLists.txt` install only XDG autostart `.desktop` files without overwriting Rust `ucc-desktop-tray` and `ucc-session-agent`;
- `KF6WindowSystem` and `KF6StatusNotifierItem` are neither searched nor linked.

Local convenience target (builds Rust binaries in `target/` and Qt6 GUI in `build/`):

```bash
make build
```

### CMake Options (`CMakeLists.txt`)

| Option | Default | Purpose |
| --- | --- | --- |
| `BUILD_GUI` | `ON` | Build the main `ucc-gui` application |
| `BUILD_CLI` | `ON` | Build legacy C++ CLI (skipped when `UCC_PACKAGE_RUST_RUNTIME=ON` and `BUILD_TESTS=OFF`) |
| `BUILD_DESKTOP_TRAY` | `ON` | Build `ucc-desktop-support` and install `ucc-desktop-tray.desktop` |
| `BUILD_TRAY` | `OFF` | Retired Plasma applet (requires `ECM`/`KF6`/`Plasma`); disabled by default (`ADR-03`) |
| `BUILD_SESSION_AGENT` | `ON` | Install `ucc-session-agent.desktop` (and build `ucc-session-support` for tests) |
| `BUILD_GNOME` | `ON` | Install the GNOME Shell extension (`ucc@uniwill.com`) |
| `BUILD_TESTS` | `OFF` | Enable CTest (`Qt6::Test`, C++/Python/GNOME/interop suites) |
| `UCC_PACKAGE_RUST_RUNTIME` | `OFF` | Package Qt6 GUI and system integration files on top of the Rust runtime binaries |
| `UCC_READ_ONLY_PREVIEW` | `OFF` | Build preview clients with all hardware and configuration write paths blocked |
| `UCC_AUTOSTART_DIR` | `${CMAKE_INSTALL_FULL_SYSCONFDIR}/xdg/autostart` | Destination directory for XDG autostart `.desktop` files |

### Read-Only Preview Build (`-DUCC_READ_ONLY_PREVIEW=ON`)

Read-Only Preview allows inspecting the UI without mutating hardware registers or configuration files. Build it in a dedicated out-of-tree directory:

```bash
cmake -S . -B /tmp/ucc-build-preview -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DUCC_READ_ONLY_PREVIEW=ON \
  -DUCC_PACKAGE_RUST_RUNTIME=ON \
  -DBUILD_TRAY=OFF \
  -DBUILD_CLI=OFF \
  -DBUILD_GNOME=OFF \
  -DBUILD_TESTS=ON
cmake --build /tmp/ucc-build-preview --target ucc-gui --parallel 2
/tmp/ucc-build-preview/bin/ucc-gui
```

For quick local execution, `./run-preview.sh` configures `build-preview/` with `CMAKE_BUILD_TYPE=Debug` and launches `ucc-gui`. Never install a preview build into `/usr` over a production package.

---

## 4. `cargo xtask` Automation

`.cargo/config.toml` defines the alias `xtask = "run --package xtask --"`. In offline environments, pass `--locked --offline` via `cargo run -p xtask --locked --offline -- <command>` or invoke the compiled `<target-dir>/release/xtask <command>` directly.

| Command | Purpose |
| --- | --- |
| `cargo run -p xtask -- dist [dest [bin-dir]]` | Stages the full Rust runtime into `dest` (default `target/dist`): 7 binaries, man pages (`ucc-cli.1`, `uccd.8`), shell completions, systemd/D-Bus/Polkit files, `uccd-pre-sleep.service -> uccd-sleep.service` symlink, `share/doc/ucc/RELEASE_MANIFEST.json` (SHA-256 checksums), and CycloneDX 1.5 `share/doc/ucc/SBOM.json` |
| `cargo run -p xtask -- install-runtime <dest> <bin-dir>` | Installs the 7 Rust binaries into `<dest>/bin` and `<dest>/lib/ucc`, plus `ucc-cli.1`, `uccd.8`, and bash/zsh/fish completions; used by `PKGBUILD`, `debian/rules`, `ucc.spec`, and `package.nix` |
| `cargo run -p xtask -- check-versions` | Verifies exact SemVer parity across all 6 manifests: `Cargo.toml`, `CMakeLists.txt`, `PKGBUILD`, `ucc.spec`, `package.nix`, `debian/changelog` |
| `cargo run -p xtask -- parser-check` | Verifies 6-manifest version parity, JSON validity and duplicate-key absence across 11 repository metadata files, CycloneDX SBOM validity, and in-memory offline-importer invariants |
| `cargo run -p xtask -- release <X.Y.Z[-pre]> [--dry-run]` | Updates the version across all 6 manifests (resetting `pkgrel=1` and `Release: 1%{?dist}`) and refreshes `CHANGELOG.md` if `git-cliff` is installed; `--dry-run` prints files without modifying them |
| `cargo run -p xtask -- completions [out-dir]` | Generates `ucc-cli.bash`, `_ucc-cli`, and `ucc-cli.fish` (default `target/completions`) |
| `cargo run -p xtask -- manpages [out-dir]` | Generates `ucc-cli.1` (default `target/man`) |
| `cargo run -p xtask -- dist-cli` (alias `assets`) | Generates both shell completions (`target/completions`) and man pages (`target/man`) |
| `cargo run -p xtask -- build-fixtures` (alias `fixtures`) | Compiles `LD_PRELOAD` C fixtures (`stand-fixtures/`, `extra-fixtures/`) into `$UCC_FIXTURES_DIR` or `/tmp/ucc-fixtures-<pid>` |
| `cargo run -p xtask -- import-offline ...` | Offline conversion of user `Data/` and TCC configurations without touching system paths |
| `cargo run -p xtask -- maintenance <prepare-deployment\|deploy\|desktop-transition>` | Deployment bundle preparation, install/rollback (`--install`/`--rollback`), and user-session transition |
| `cargo run -p xtask -- capture-gallery --build <dir> [--output <dir>]` | Captures and verifies the GUI screenshot gallery on an isolated D-Bus session |

---

## 5. Distribution Packaging and Manual Installation

### Arch Linux (`PKGBUILD`)

Package: `ucc-rusted` (`epoch=1`, `provides=("uniwill-control-center=$epoch:$pkgver" "frixit-control-center=$epoch:$pkgver")`, `conflicts=('uniwill-control-center' 'uniwill-control-center-mechrevo' 'frixit-control-center')`, `replaces=('frixit-control-center')`, install hook `ucc.install`).

Building from a local checkout:

```bash
make arch
# or directly:
UCC_SOURCE_DIR="$PWD" makepkg -sf --noconfirm
sudo pacman -U ./dist/arch/ucc-rusted-*.pkg.tar.zst
```

The `check()` stage in `PKGBUILD` automatically runs `dbus-run-session -- cargo test --workspace --locked --offline --target-dir "$srcdir/rust"`, builds `interop_daemon`, executes `ctest --test-dir "$srcdir/build" -j1 --output-on-failure` with `UCC_INTEROP_DAEMON_BIN`, and runs `python3 contrib/mechrevo/tests/test_migration.py`.

### Debian / Ubuntu (`debian/*`)

Builds from the vendored offline archive (`make dist` -> `debuild -uc -us`):

```bash
make deb
```

`debian/rules` runs `dh $@ -Scmake` with `DEB_BUILD_MAINT_OPTIONS=hardening=+all`, configures the standalone Qt6 GUI (`-DCMAKE_BUILD_TYPE=RelWithDebInfo -DUCC_PACKAGE_RUST_RUNTIME=ON -DBUILD_TRAY=OFF -DBUILD_CLI=OFF`), builds Rust binaries via `cargo build --workspace --release --locked --offline --bins --target-dir target/deb`, and installs them with `xtask install-runtime debian/tmp/usr target/deb/release`.

### Fedora / RPM (`ucc.spec`)

Builds binary RPM and SRPM packages from the vendored archive:

```bash
make rpm    # outputs: dist/rpmbuild/RPMS/ and dist/rpmbuild/SRPMS/
make srpm   # SRPM only
```

`ucc.spec` enables `%global _hardened_build 1`, declares `Provides: ucc-rusted = %{epoch}:%{version}-%{release}`, compiles Rust binaries into `target/rpm`, and installs them via `xtask install-runtime "%{buildroot}%{_prefix}" target/rpm/release`.

### Nix / NixOS (`flake.nix`, `package.nix`, `nix/nixos-module.nix`)

Local build and flake checks:

```bash
nix build .#ucc
nix flake check --print-build-logs
nix develop   # devShell with cmake, extra-cmake-modules, pkg-config, deadnix, nixfmt, statix, treefmt
```

`package.nix` imports `Cargo.lock` via `rustPlatform.importCargoLock`, builds the Qt6 GUI with `-DUCC_AUTOSTART_DIR=etc/xdg/autostart`, wraps `ucc-gui` (`wrapQtApp`) and `uccd` (`wrapProgram` with runtime `PATH`), and patches `/usr/bin/uccd`, `/usr/lib/ucc/ucc-hardware`, and `/usr/lib/ucc/ucc-sleep` in systemd and D-Bus unit files to `$out/...`.

Using the NixOS module in a system `flake.nix`:

```nix
{
  inputs.ucc.url = "github:Frixlt/UCC";

  outputs = { nixpkgs, ucc, ... }: {
    nixosConfigurations.laptop = nixpkgs.lib.nixosSystem {
      modules = [
        ucc.nixosModules.default
        {
          services.uccd = {
            enable = true;
            enableSleepHandler = true;
            extraArgs = [ ]; # e.g., [ "--verbose" ]
          };
        }
      ];
    };
  };
}
```

### Manual Installation (Without a Package Manager)

Do not mix manual installation into `/usr` with distribution-managed packages.

```bash
sudo cmake --install /tmp/ucc-build-normal
sudo target/release/xtask install-runtime /usr target/release
sudo install -d -m 0755 /etc/ucc
sudo systemctl daemon-reload
sudo systemctl reload dbus
sudo systemctl enable --now uccd.service
sudo systemctl enable uccd-sleep.service
```

---

## 6. Installed Filesystem Layout

| Path | Component / Purpose |
| --- | --- |
| `/usr/bin/uccd` | System daemon `com.uniwill.uccd` (Rust) |
| `/usr/bin/ucc-gui` | Main GUI application (C++20 / `Qt6::Widgets` + `Qt6::Charts`) |
| `/usr/bin/ucc-cli` | Command-line utility (Rust) |
| `/usr/bin/ucc-desktop-tray` | StatusNotifierItem system tray (Rust) |
| `/usr/bin/ucc-session-agent` | User-session notification, Lightbar, and refresh-rate agent (Rust) |
| `/usr/lib/ucc/ucc-hardware` | Root helper for sensor audit (`--audit`) and emergency fan recovery (`--fan-auto`) |
| `/usr/lib/ucc/ucc-sleep` | Root helper for suspend/resume state handoff (`prepare` / `resume`) |
| `/usr/lib/ucc/ucc-import-check` | Offline configuration importer and schema validator |
| `/usr/lib/systemd/system/uccd.service` | Main daemon systemd unit (`Type=dbus`, `ExecStopPost=-/usr/lib/ucc/ucc-hardware --fan-auto`) |
| `/usr/lib/systemd/system/uccd-sleep.service` | Sleep hook unit (`RequiredBy=sleep.target`, `Before=sleep.target nvidia-suspend.service nvidia-hibernate.service`) |
| `/usr/lib/systemd/system/uccd-pre-sleep.service` | Alias unit / symlink to `uccd-sleep.service` |
| `/usr/share/dbus-1/system-services/com.uniwill.uccd.service` | System D-Bus activation descriptor |
| `/usr/share/dbus-1/system.d/com.uniwill.uccd.conf` | System D-Bus access policy |
| `/usr/share/polkit-1/actions/com.uniwill.uccd.policy` | Polkit authorization policy for privileged actions |
| `/etc/xdg/autostart/ucc-desktop-tray.desktop` | XDG autostart entry for the tray |
| `/etc/xdg/autostart/ucc-session-agent.desktop` | XDG autostart entry for the session agent |
| `/etc/ucc/` | System configuration directory (`/etc/ucc/settings`, optional `/etc/ucc/aquaris.json`) |
| `/usr/share/applications/ucc-gui.desktop` | Desktop launcher for `ucc-gui` |
| `/usr/share/icons/hicolor/{scalable,16x16..128x128}/apps/ucc-{gui,tray}.{svg,png}` | GUI and tray icons |
| `/usr/share/man/man1/ucc-cli.1` | CLI man page |
| `/usr/share/man/man8/uccd.8` | Daemon `uccd` man page |
| `/usr/share/bash-completion/completions/ucc-cli` | Bash completion |
| `/usr/share/zsh/site-functions/_ucc-cli` | Zsh completion |
| `/usr/share/fish/vendor_completions.d/ucc-cli.fish` | Fish completion |
| `/usr/share/gnome-shell/extensions/ucc@uniwill.com/` | GNOME Shell extension (`metadata.json`, `extension.js`, `uccdClient.js`, `stylesheet.css`) |
| `/usr/share/doc/ucc/` | Documentation, licenses, `RELEASE_MANIFEST.json`, and `SBOM.json` (via `xtask dist`) |
