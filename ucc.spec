%global _hardened_build 1

Name:           ucc
Version:        0.3.0
Release:        1%{?dist}
Summary:        ucc-rusted - System control application suite for Uniwill/Clevo/TUXEDO laptops
License:        GPL-3.0-or-later

URL:            https://github.com/Frixlt/UCC
Source0:        https://github.com/Frixlt/UCC/releases/download/v%{version}/ucc-rusted-%{version}.tar.gz
Provides:       ucc-rusted = %{version}-%{release}

BuildRequires:  cmake >= 3.20
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc-c++
BuildRequires:  nlohmann-json-devel
BuildRequires:  qt6-qtbase-devel >= 6.0
BuildRequires:  qt6-qtcharts-devel
BuildRequires:  systemd-devel
BuildRequires:  libXrandr-devel
BuildRequires:  libgudev-devel

Requires:       systemd
Requires:       qt6-qtbase >= 6.0
Requires:       qt6-qtcharts
Requires:       tuxedo-drivers
Requires:       polkit
Requires(post): systemd
Requires(preun): systemd
Requires(postun): systemd

%description
ucc-rusted (Uniwill Control Center) is a Rust-backed system control application
suite for Uniwill/Clevo/TUXEDO laptops. It provides:

- Daemon (uccd): Background service for system control and monitoring
- GUI (ucc-gui): Main graphical user interface
- CLI (ucc-cli): Command-line interface for scripting and headless use
- System tray client for quick access

Features include fan control, keyboard backlight management, display settings,
CPU power management, and water cooler control for supported systems.

%prep
%autosetup -n ucc-%{version}

%build
cargo build --workspace --release --locked --offline --bins --target-dir target/rpm
%cmake \
  -DCMAKE_BUILD_TYPE=Release \
  -DCMAKE_INSTALL_PREFIX=/usr \
  -DCMAKE_INSTALL_LIBDIR=%{_libdir} \
  -DCMAKE_INSTALL_RPATH="" \
  -DBUILD_GUI=ON \
  -DBUILD_TRAY=OFF \
  -DBUILD_GNOME=ON \
  -DBUILD_CLI=OFF \
  -DUCC_PACKAGE_RUST_RUNTIME=ON
%cmake_build -j

%install
%cmake_install
target/rpm/release/xtask install-runtime "%{buildroot}%{_prefix}" target/rpm/release

# Create configuration directory
mkdir -p %{buildroot}%{_sysconfdir}/ucc

# Set permissions
chmod 755 %{buildroot}%{_sysconfdir}/ucc

%post
%systemd_post uccd.service
systemctl daemon-reload > /dev/null 2>&1 || true

%preun
%systemd_preun uccd.service

%postun
%systemd_postun_with_restart uccd.service

%files
%license LICENSE
%{_docdir}/ucc/
%{_bindir}/uccd
%{_bindir}/ucc-gui
%{_bindir}/ucc-cli
%{_bindir}/ucc-desktop-tray
%{_bindir}/ucc-session-agent
/usr/lib/ucc/ucc-hardware
/usr/lib/ucc/ucc-sleep
/usr/lib/ucc/ucc-import-check
%{_sysconfdir}/xdg/autostart/ucc-desktop-tray.desktop
%{_sysconfdir}/xdg/autostart/ucc-session-agent.desktop
%{_unitdir}/uccd.service
%{_unitdir}/uccd-pre-sleep.service
%{_unitdir}/uccd-sleep.service
%{_datadir}/dbus-1/system-services/com.uniwill.uccd.service
%{_datadir}/dbus-1/system.d/com.uniwill.uccd.conf
%{_datadir}/polkit-1/actions/com.uniwill.uccd.policy
%dir %{_sysconfdir}/ucc
%{_datadir}/applications/ucc-gui.desktop
# Icons (SVG and PNG, all sizes)
%{_datadir}/icons/hicolor/scalable/apps/ucc-gui.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-gui_16.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-gui_24.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-gui_32.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-gui_48.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-gui_64.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-gui_128.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-tray.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-tray_16.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-tray_24.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-tray_32.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-tray_48.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-tray_64.svg
%{_datadir}/icons/hicolor/scalable/apps/ucc-tray_128.svg
%{_datadir}/icons/hicolor/16x16/apps/ucc-gui.png
%{_datadir}/icons/hicolor/24x24/apps/ucc-gui.png
%{_datadir}/icons/hicolor/32x32/apps/ucc-gui.png
%{_datadir}/icons/hicolor/48x48/apps/ucc-gui.png
%{_datadir}/icons/hicolor/64x64/apps/ucc-gui.png
%{_datadir}/icons/hicolor/128x128/apps/ucc-gui.png
%{_datadir}/icons/hicolor/16x16/apps/ucc-tray.png
%{_datadir}/icons/hicolor/24x24/apps/ucc-tray.png
%{_datadir}/icons/hicolor/32x32/apps/ucc-tray.png
%{_datadir}/icons/hicolor/48x48/apps/ucc-tray.png
%{_datadir}/icons/hicolor/64x64/apps/ucc-tray.png
%{_datadir}/icons/hicolor/128x128/apps/ucc-tray.png
%{_mandir}/man1/ucc-cli.1*
%{_datadir}/bash-completion/completions/ucc-cli
%{_datadir}/zsh/site-functions/_ucc-cli
%{_datadir}/fish/vendor_completions.d/ucc-cli.fish
# GNOME Shell extension
%{_datadir}/gnome-shell/extensions/ucc@uniwill.com/metadata.json
%{_datadir}/gnome-shell/extensions/ucc@uniwill.com/extension.js
%{_datadir}/gnome-shell/extensions/ucc@uniwill.com/uccdClient.js
%{_datadir}/gnome-shell/extensions/ucc@uniwill.com/stylesheet.css
