# ucc-gui/AGENTS.md

Scoped rules loaded when editing `ucc-gui/` or `libucc-dbus/`.

## Architecture Constraints (`ADR-01`, `ADR-03`)
- On-demand C++20 `Qt6::Widgets` + `Qt6::Charts` D-Bus client; exits completely on window close (0 MB Qt in background).
- Allowed links ONLY: `Qt6::{Core,Gui,Widgets,DBus,Charts}` + statically linked `libucc-dbus`.
- **Forbidden:** `KF6*`, `Plasma`, `Qt6Qml`, `Qt6Quick`, `Qt6Bluetooth`, public `libucc-dbus.so`.
- **UI thread discipline:** never add synchronous `QDBusInterface::call()` or `QProcess::waitForFinished()` on the GUI thread; use `AsyncRead.hpp` / `QDBusPendingCallWatcher`.
- **Qt lifetimes:** always guard lambdas capturing `this` with `QPointer` or pass `this` as receiver context; never `delete` a `QGraphicsItem` synchronously inside its own `mousePressEvent()`.
- **D-Bus JSON types:** round fan/pump curve coordinates (`qRound`) to integers before sending to `ApplyFanProfiles`.

## Build, `ldd` & Interop Test Commands
```bash
export PATH="$HOME/.cargo/bin:$PATH"
cargo build -q -p uccd --example interop_daemon --locked --offline
cmake -S . -B /tmp/ucc-build -G Ninja -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DCMAKE_CXX_FLAGS="-D_GLIBCXX_ASSERTIONS" \
  -DUCC_PACKAGE_RUST_RUNTIME=ON -DBUILD_TRAY=OFF -DBUILD_TESTS=ON
cmake --build /tmp/ucc-build --parallel
! ldd /tmp/ucc-build/bin/ucc-gui | grep -qE 'not found|KF6|Qt6Qml|Qt6Quick|Qt6Bluetooth|libucc-dbus'
UCC_INTEROP_DAEMON_BIN="$PWD/target/debug/examples/interop_daemon" \
  ctest --test-dir /tmp/ucc-build --output-on-failure --parallel
```
