# Первоначальный переход с Python/TCC на UCC

Состояние процедуры: переход уже выполнен на основной машине. Эта инструкция
описывает специальный first-install bundle с личными данными. Публичный релиз
0.3.0 собирается без этих данных; обновление UCC описано в
[BUILDING.md](../../docs/BUILDING.md). Не публикуйте private deployment bundle.

This procedure replaces the running Python ControlCenter and TCC controllers with
the fork. The kernel driver and original repositories are retained. It is for a
first UCC installation; the installer refuses existing UCC files/packages.

Run complete normal and preview CTest builds, keeping JUnit reports. Normal builds
must use `/usr` as their install prefix and have the GUI, CLI, standalone tray and
session agent enabled. Then, as the desktop user:

```sh
python3 contrib/frixit/prepare-deployment.py \
  --build /path/to/normal-build \
  --data /path/to/ControlCenter/Data \
  --normal-tests /path/to/normal-tests.xml \
  --preview-tests /path/to/preview-tests.xml \
  ./artifacts/new-deployment
```

The private bundle contains fresh source snapshots, physical keyboard capabilities,
the converted settings, parser validation, both test reports, a tracked Arch
package and its SHA-256 manifest. It does not change the running system. Review
`import/report.json`, `deployment.json` and `package-info.txt` in that bundle.
Missing original profiles or failing test reports prevent preparation.

An explicit `--allow-gui-audit-failure` option permits a user-requested launch
when `test_gui_audit` is the only failing CTest entry. The failing reports and
exception remain recorded in `deployment.json`; this does not mark the audit as
passing. Other failing tests and empty reports still prevent preparation.

```sh
python3 contrib/frixit/desktop-transition.py quiesce ./artifacts/desktop-backup \
  --source /path/to/ControlCenter
pkexec /usr/bin/python3 ./artifacts/new-deployment/deploy-system.py \
  --install ./artifacts/new-deployment
python3 contrib/frixit/desktop-transition.py activate ./artifacts/desktop-backup \
  --preferences ./artifacts/new-deployment/import/desktop-preferences.json
```

The root step installs Qt Charts from the configured package repositories, then
installs `frixit-control-center` through pacman. Before stopping existing
controllers it saves their enabled/active states, TCC configuration and source
inputs under `/var/lib/ucc/deployments/<id>`. A source checksum change aborts the
transition. It validates the installed configuration, disables the old root
controllers/sleep hook and temporarily masks TCC activation. The new daemon and
sleep hook are enabled. The installer checks device support and power-policy
access, then exercises `ucc-sleep prepare/resume` without suspending the machine.

The desktop step retains the old autostart entry in its private backup and replaces
it with a Hidden override, imports the original appearance/minimized preferences,
and starts the installed tray, notification agent and GUI as the desktop user.
Other applications are untouched. Global tray/agent autostart entries handle
subsequent logins.

## Verification

```sh
systemctl status uccd.service
systemctl is-enabled uccd.service uccd-sleep.service
ucc-cli --json status
ucc-cli power status
ucc-cli devices status
ucc-cli lightbar status
pgrep -af '/usr/bin/ucc-(desktop-tray|session-agent|gui)'
journalctl -u uccd.service -b
```

Compare live keyboard colors and the device/lightbar settings with the prepared
import. Missing disconnected devices remain listed and are reported separately.
The installed desktop autostart entries are in `/etc/xdg/autostart`; current
transient user-unit names can vary. A real hardware sleep cycle and long-duration
external cooler reconnection still require observations on this laptop. The installer never
automatically suspends, changes display resolution or sends termination signals
to GPU applications.

## Recovery

Failures during the root installation trigger automatic rollback. For an installed
deployment, use the backup path printed by the installer:

```sh
pkexec /usr/bin/python3 /var/lib/ucc/deployments/<id>/deploy-system.py \
  --rollback /var/lib/ucc/deployments/<id>
python3 contrib/frixit/desktop-transition.py restore ./artifacts/desktop-backup
```

Rollback stops/disables UCC, removes its tracked package, retains UCC settings in
the backup and restores the old services' previous states. The desktop restore
recovers the old autostart/preferences and restarts the original tray/lightbar
launcher when it previously ran. Qt Charts is retained as an ordinary dependency;
rollback does not remove shared system libraries. Original TCC/Python files are
never deleted.
