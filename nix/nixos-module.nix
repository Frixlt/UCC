{
  overlay ? null,
}:
{
  config,
  lib,
  pkgs,
  ...
}:

let
  cfg = config.services.uccd;
  extraArgsString = lib.concatStringsSep " " (map lib.escapeShellArg cfg.extraArgs);
  videoDrivers = config.services.xserver.videoDrivers or [ ];
  nvidiaPackage = lib.attrByPath [ "hardware" "nvidia" "package" ] null config;
  hasNvidia = nvidiaPackage != null && lib.elem "nvidia" videoDrivers;
  nvidiaBinPath = lib.optionalString hasNvidia ":${lib.makeBinPath [ nvidiaPackage ]}";
  # libnvidia-ml.so.1 is loaded via dlopen() at runtime; NixOS exposes it under
  # /run/opengl-driver/lib which is always set up when nvidia drivers are enabled.
  nvidiaLibPath = lib.optionalString hasNvidia "/run/opengl-driver/lib";
  uccdToolsPath = lib.makeBinPath [
    pkgs.coreutils
    pkgs.gawk
    pkgs.gnugrep
    pkgs.procps
    pkgs.util-linux
    pkgs.which
  ];
in
{
  options.services.uccd = {
    enable = lib.mkEnableOption "Uniwill Control Center daemon (uccd)";

    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.ucc or (pkgs.callPackage ../package.nix { src = ../.; });
      defaultText = "pkgs.ucc (or callPackage ../package.nix)";
      description = "The `ucc` package providing `uccd`.";
    };

    extraArgs = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "--verbose" ];
      description = "Extra arguments passed to `uccd --start`.";
    };

    enableSleepHandler = lib.mkOption {
      type = lib.types.bool;
      default = true;
      description = "Preserve and restore UCC state on suspend and hibernate.";
    };
  };

  config = lib.mkIf cfg.enable {
    nixpkgs.overlays = lib.mkIf (overlay != null) [ overlay ];

    # Add the package to the system profile so plasmashell can discover the
    # Plasma applet .so (via QT_PLUGIN_PATH) and the QML module
    # (com.uniwill.ucc.private, via QML2_IMPORT_PATH / QT_QML_IMPORT_PATH).
    environment.systemPackages = [ cfg.package ];

    # Install the Polkit policy so that unprivileged GUI users can
    # authenticate for privileged D-Bus operations.
    security.polkit.enable = true;

    # Make the D-Bus service config and Polkit policy discoverable at the
    # system level.  environment.systemPackages alone is not sufficient for
    # system-bus configs and Polkit action definitions.
    services.dbus.packages = [ cfg.package ];
    environment.pathsToLink = [ "/share/polkit-1" ];

    environment.etc = {
      "xdg/autostart/ucc-desktop-tray.desktop".source =
        "${cfg.package}/etc/xdg/autostart/ucc-desktop-tray.desktop";
      "xdg/autostart/ucc-session-agent.desktop".source =
        "${cfg.package}/etc/xdg/autostart/ucc-session-agent.desktop";
    };

    systemd.tmpfiles.rules = [
      "d /etc/ucc 0755 root root - -"
    ];

    systemd.services.uccd = {
      description = "Uniwill Control Center Daemon";
      documentation = [ "man:uccd(8)" ];
      after = [ "dbus.service" ];
      requires = [ "dbus.service" ];
      wantedBy = [ "multi-user.target" ];

      serviceConfig = {
        Type = "dbus";
        BusName = "com.uniwill.uccd";
        ExecStartPre = "-${cfg.package}/bin/uccd --stop";
        ExecStart =
          "${cfg.package}/bin/uccd --start" + lib.optionalString (cfg.extraArgs != [ ]) " ${extraArgsString}";
        ExecStopPost = "-${cfg.package}/lib/ucc/ucc-hardware --fan-auto";
        Environment = [
          "PATH=/run/wrappers/bin:/run/current-system/sw/bin:${uccdToolsPath}${nvidiaBinPath}"
        ]
        ++ lib.optionals hasNvidia [
          "LD_LIBRARY_PATH=${nvidiaLibPath}"
        ];
        Restart = "on-failure";
        RestartSec = "5s";
        TimeoutStopSec = "10s";
        User = "root";
        StandardOutput = "journal";
        StandardError = "journal";
        SyslogIdentifier = "uccd";

        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        NoNewPrivileges = true;
        ReadWritePaths = [
          "/etc/ucc"
          "/run"
          "/sys/devices"
          "-/proc/sys/kernel/nmi_watchdog"
          "-/proc/sys/vm/dirty_writeback_centisecs"
          "-/sys/module/snd_hda_intel/parameters/power_save"
          "-/sys/module/pcie_aspm/parameters/policy"
        ];
      };
    };

    systemd.services.uccd-sleep = lib.mkIf cfg.enableSleepHandler {
      description = "Preserve UCCD runtime settings across system sleep";
      after = [ "dbus.service" ];
      before = [
        "sleep.target"
        "nvidia-suspend.service"
        "nvidia-hibernate.service"
      ];
      requiredBy = [ "sleep.target" ];

      unitConfig = {
        DefaultDependencies = false;
        StopWhenUnneeded = true;
      };
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${cfg.package}/lib/ucc/ucc-sleep prepare";
        ExecStopPost = "${cfg.package}/lib/ucc/ucc-sleep resume";
        Environment = [ "UCC_SYSTEMCTL=${config.systemd.package}/bin/systemctl" ];
        TimeoutStartSec = "60s";
        TimeoutStopSec = "30s";
        User = "root";
        UMask = "0077";
      };
    };
  };
}
