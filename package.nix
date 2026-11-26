{
  lib,
  cmake,
  coreutils,
  gawk,
  kdePackages,
  gnugrep,
  procps,
  util-linux,
  which,
  stdenv,
  cargo,
  rustc,
  rustPlatform,
  nlohmann_json,
  pkg-config,
  libxrandr,
  libxcb-cursor,
  systemd,
  makeWrapper,
  tuxedo-drivers ? null,
  src ? ./., # default to the source tree containing this file
  version ? "0.3.0",
}:

stdenv.mkDerivation {
  pname = "ucc";
  inherit version;

  inherit src;

  cargoDeps = rustPlatform.importCargoLock {
    lockFile = "${src}/Cargo.lock";
  };

  nativeBuildInputs = [
    cmake
    cargo
    rustc
    rustPlatform.cargoSetupHook
    pkg-config
    makeWrapper
    kdePackages.wrapQtAppsHook
  ];

  buildInputs = [
    kdePackages.qtbase
    kdePackages.qtcharts
    kdePackages.qtwayland
    kdePackages.plasma-integration
    kdePackages.breeze
    nlohmann_json
    libxrandr
    libxcb-cursor
    systemd
  ]
  ++ lib.optionals (tuxedo-drivers != null) [
    tuxedo-drivers
  ];

  cmakeFlags = [
    "-DCMAKE_BUILD_TYPE=Release"
    "-DBUILD_GUI=ON"
    "-DBUILD_TRAY=OFF"
    "-DBUILD_GNOME=ON"
    "-DBUILD_CLI=OFF"
    "-DUCC_PACKAGE_RUST_RUNTIME=ON"
    "-DUCC_AUTOSTART_DIR=etc/xdg/autostart"
  ];

  postPatch = ''
    rustSourceRoot="$PWD"
  '';

  postBuild = ''
    (cd "$rustSourceRoot" &&
      cargo build --workspace --bins --release --locked --offline --target-dir "$NIX_BUILD_TOP/rust-target")
  '';

  postInstall = ''
    "$NIX_BUILD_TOP/rust-target/release/xtask" install-runtime "$out" "$NIX_BUILD_TOP/rust-target/release"
  '';

  dontWrapQtApps = true;
  postFixup = ''
    wrapQtApp "$out/bin/ucc-gui"
    wrapProgram $out/bin/uccd \
      --prefix PATH : "${
        lib.makeBinPath [
          coreutils
          gawk
          gnugrep
          procps
          util-linux
          which
        ]
      }"

    # The upstream unit/DBus activation files use /usr/bin paths which do not
    # exist on NixOS. Keep the files installed by CMake, but patch paths to the
    # store so they are usable via `systemd.packages` / `services.dbus.packages`.
    if [ -f "$out/lib/systemd/system/uccd.service" ]; then
      substituteInPlace "$out/lib/systemd/system/uccd.service" \
        --replace-fail "/usr/bin/uccd" "$out/bin/uccd"
    fi
    if [ -f "$out/share/dbus-1/system-services/com.uniwill.uccd.service" ]; then
      substituteInPlace "$out/share/dbus-1/system-services/com.uniwill.uccd.service" \
        --replace-fail "/usr/bin/uccd" "$out/bin/uccd"
      if ! grep -q '^SystemdService=uccd\\.service$' "$out/share/dbus-1/system-services/com.uniwill.uccd.service"; then
        sed -i '/^Name=com\\.uniwill\\.uccd$/a SystemdService=uccd.service' \
          "$out/share/dbus-1/system-services/com.uniwill.uccd.service"
      fi
    fi
    if [ -f "$out/lib/systemd/system/uccd.service" ] && grep -q '/usr/lib/ucc/ucc-hardware' "$out/lib/systemd/system/uccd.service"; then
      substituteInPlace "$out/lib/systemd/system/uccd.service" \
        --replace-fail "/usr/lib/ucc/ucc-hardware" "$out/lib/ucc/ucc-hardware"
    fi
    substituteInPlace "$out/lib/systemd/system/uccd-sleep.service" \
      --replace-fail "/usr/lib/ucc/ucc-sleep" "$out/lib/ucc/ucc-sleep" \
      --replace-fail "[Service]" "$(printf '[Service]\nEnvironment=UCC_SYSTEMCTL=%s/bin/systemctl' "${systemd}")"

    if [ -f "$out/lib/systemd/system/uccd.service" ]; then
      if ! grep -q '^Type=dbus$' "$out/lib/systemd/system/uccd.service"; then
        sed -i 's/^Type=simple$/Type=dbus/' "$out/lib/systemd/system/uccd.service"
      fi
      if ! grep -q '^BusName=com\\.uniwill\\.uccd$' "$out/lib/systemd/system/uccd.service"; then
        sed -i '/^Type=dbus$/a BusName=com.uniwill.uccd' "$out/lib/systemd/system/uccd.service"
      fi
    fi
  '';

  meta = with lib; {
    description = "Uniwill Control Center - System control application suite (daemon, GUI, CLI, tray)";
    homepage = "https://github.com/Frixlt/UCC";
    license = licenses.gpl3Plus;
    platforms = platforms.linux;
    maintainers = [ ];
  };
}
