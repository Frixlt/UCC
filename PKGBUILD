# ucc-rusted release; no private settings or kernel driver bundled.
pkgname=ucc-rusted
pkgver=0.0.1
pkgrel=1
pkgdesc='ucc-rusted: Rust system control daemon, CLI, tray, and Qt6 GUI for Uniwill/Clevo/TUXEDO laptops'
arch=('x86_64')
url='https://github.com/Frixlt/UCC'
license=('GPL-3.0-or-later')
depends=('qt6-base' 'qt6-charts' 'systemd-libs' 'dbus' 'polkit' 'hicolor-icon-theme' 'libxcb')
makedepends=('cmake' 'ninja' 'git' 'pkgconf' 'nlohmann-json' 'qt6-connectivity' 'rust')
checkdepends=('python' 'gjs' 'nodejs')
optdepends=('tuxedo-drivers: hardware interface required by uccd'
            'bluez: Bluetooth water cooler'
            'kscreen: KDE refresh-rate offers'
            'qt6-wayland: native Wayland desktop clients')
provides=("uniwill-control-center=$pkgver" "frixit-control-center=$pkgver")
conflicts=('uniwill-control-center' 'uniwill-control-center-mechrevo' 'frixit-control-center')
replaces=('frixit-control-center')
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

prepare() {
  local source_root=${UCC_SOURCE_DIR:-$srcdir/UCC-$pkgver}
  (cd "$source_root" && cargo fetch --locked)
}

build() {
  local source_root=${UCC_SOURCE_DIR:-$srcdir/UCC-$pkgver}
  (cd "$source_root" && cargo build --workspace --bins --release --locked --offline --target-dir "$srcdir/rust")
  cmake -S "$source_root" -B "$srcdir/build" -G Ninja \
    -DCMAKE_BUILD_TYPE=Release -DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_INSTALL_LIBDIR=lib \
    -DBUILD_GUI=ON -DBUILD_CLI=ON -DBUILD_DESKTOP_TRAY=ON -DBUILD_SESSION_AGENT=ON \
    -DBUILD_TRAY=OFF -DBUILD_GNOME=ON -DBUILD_TESTS=ON -DUCC_PACKAGE_RUST_RUNTIME=ON
  cmake --build "$srcdir/build" -j2
}

check() {
  local source_root=${UCC_SOURCE_DIR:-$srcdir/UCC-$pkgver}
  (cd "$source_root" && dbus-run-session -- cargo test --workspace --locked --offline --target-dir "$srcdir/rust")
  (cd "$source_root" && cargo build -p uccd --example interop_daemon --locked --offline --target-dir "$srcdir/rust")
  CARGO_TARGET_DIR="$srcdir/rust" \
    UCC_INTEROP_DAEMON_BIN="$srcdir/rust/debug/examples/interop_daemon" \
    ctest --test-dir "$srcdir/build" -j1 --output-on-failure
  python3 "$source_root/contrib/mechrevo/tests/test_migration.py"
}

package() {
  DESTDIR="$pkgdir" cmake --install "$srcdir/build"
  "$srcdir/rust/release/xtask" install-runtime "$pkgdir/usr" "$srcdir/rust/release"
  install -dm755 "$pkgdir/etc/ucc"
  install -Dm644 "${UCC_SOURCE_DIR:-$srcdir/UCC-$pkgver}/COPYING" \
    "$pkgdir/usr/share/licenses/$pkgname/COPYING"
}
