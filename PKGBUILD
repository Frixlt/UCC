# Frixit Control Center release; no private settings or kernel driver bundled.
pkgname=frixit-control-center
pkgver=0.3.0
pkgrel=1
pkgdesc='Frixit Control Center: personal ControlCenter behavior integrated with UCC Revorked'
arch=('x86_64')
url='https://github.com/Frixlt/UCC'
license=('GPL-3.0-or-later')
depends=('qt6-base' 'qt6-charts' 'qt6-connectivity' 'qt6-declarative'
         'systemd-libs' 'dbus' 'polkit' 'hicolor-icon-theme' 'libxcb'
         'kwindowsystem' 'kstatusnotifieritem')
makedepends=('cmake' 'ninja' 'git' 'pkgconf' 'nlohmann-json')
checkdepends=('python' 'gjs' 'nodejs')
optdepends=('tuxedo-drivers: hardware interface required by uccd'
            'bluez: Bluetooth water cooler'
            'kscreen: KDE refresh-rate offers'
            'qt6-wayland: native Wayland desktop clients')
provides=("uniwill-control-center=$pkgver")
conflicts=('uniwill-control-center' 'uniwill-control-center-mechrevo')
options=('!debug')
install=ucc.install
# Settings are created/imported locally, never shipped in the public package.
if [[ -n ${UCC_SOURCE_DIR:-} ]]; then
  source=()
  sha256sums=()
else
  source=("$pkgname-$pkgver.tar.gz::$url/archive/refs/tags/v$pkgver.tar.gz")
  sha256sums=('SKIP')
fi

build() {
  local source_root=${UCC_SOURCE_DIR:-$srcdir/UCC-$pkgver}
  cmake -S "$source_root" -B "$srcdir/build" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_INSTALL_LIBDIR=lib \
    -DBUILD_GUI=ON -DBUILD_CLI=ON -DBUILD_DESKTOP_TRAY=ON -DBUILD_SESSION_AGENT=ON \
    -DBUILD_TRAY=OFF -DBUILD_GNOME=ON -DBUILD_TESTS=ON
  cmake --build "$srcdir/build" -j2
}

check() {
  local source_root=${UCC_SOURCE_DIR:-$srcdir/UCC-$pkgver}
  ctest --test-dir "$srcdir/build" -j1 --output-on-failure -E '^test_gui_audit$'
  dbus-run-session -- bash "$source_root/tests/run_gui_audit_test.sh" \
    "$srcdir/build/bin/test_gui_audit" dashboardShowsDriverFailureAndClearsAfterRecovery \
    aboutBuildCreditsAndBundledLicense calibratedKeyboardKeepsPhysicalLedIdentities \
    lightbarKeepsSourceValuesAndRejectsFailedCommands deviceMatrixKeepsThreeIndependentModes
  python3 "$source_root/contrib/mechrevo/tests/test_migration.py"
}

package() {
  DESTDIR="$pkgdir" cmake --install "$srcdir/build"
  install -dm755 "$pkgdir/etc/ucc"
  install -Dm644 "${UCC_SOURCE_DIR:-$srcdir/UCC-$pkgver}/COPYING" \
    "$pkgdir/usr/share/licenses/$pkgname/COPYING"
}
