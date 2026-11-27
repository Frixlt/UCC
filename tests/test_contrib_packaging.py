"""Standalone unittest/pytest reproduction tests for contrib/ and tests/ findings on commit 1d07126 (N03, N22, N27, N32, R18, R19, M-EXT-10, M-PKG-06).

Matches tests/test_deployment.py conventions; no host commands or root writes.
"""
import importlib.util
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import tempfile
import unittest


def resolve_repo_root() -> Path:
    env_dir = os.environ.get('UCC_REPO_DIR')
    if env_dir and (Path(env_dir) / 'contrib/frixit/deploy-system.py').is_file():
        return Path(env_dir).resolve()
    drop_in = Path(__file__).resolve().parents[1]
    if (drop_in / 'contrib/frixit/deploy-system.py').is_file():
        return drop_in
    sibling = Path(__file__).resolve().parents[2] / 'ucc_review/UCC-rust-migration'
    if (sibling / 'contrib/frixit/deploy-system.py').is_file():
        return sibling
    raise RuntimeError('Could not locate UCC repository root (set UCC_REPO_DIR or drop into tests/)')


SOURCE = resolve_repo_root()
SPEC = importlib.util.spec_from_file_location('deploy_system', SOURCE / 'contrib/frixit/deploy-system.py')
deploy = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(deploy)


class FakeSystem:
    def __init__(self, root: Path):
        self.root = root
        self.calls = []
        self.active = {name: 'active' if name != 'tccd-sleep.service' else 'inactive' for name in deploy.OLD_UNITS}
        self.enabled = {name: 'enabled' for name in deploy.OLD_UNITS}
        self.installed = False

    def __call__(self, args, *, check=True):
        args = tuple(map(str, args))
        self.calls.append(args)
        output, code = '', 0
        if args[:2] == ('pacman', '-Q'):
            code = 0 if self.installed else 1
        elif args[:2] == ('pacman', '-U'):
            self.installed = True
            settings = self.root / 'etc/ucc/settings'
            settings.parent.mkdir(parents=True, exist_ok=True)
            settings.write_text('{}')
        elif args[:2] == ('pacman', '-R'):
            self.installed = False
        elif args[:2] == ('systemctl', 'is-active'):
            output = self.active.get(args[2], 'inactive')
        elif args[:2] == ('systemctl', 'is-enabled'):
            output = self.enabled.get(args[2], 'not-found')
        elif args[:2] in (('systemctl', 'start'), ('systemctl', 'stop')):
            for name in args[2:]:
                self.active[name] = 'active' if args[1] == 'start' else 'inactive'
        elif args[:2] in (('systemctl', 'enable'), ('systemctl', 'disable')):
            for name in args[2:]:
                self.enabled[name] = 'enabled' if args[1] == 'enable' else 'disabled'
        elif args[0] == 'busctl':
            output = 'b true'
        elif args[0] == 'ucc-cli':
            output = '{"current_mode":"ac"}'
        return subprocess.CompletedProcess(args, code, output, '')


class Repro1d07126ContribTest(unittest.TestCase):
    def test_n03_pacman_hook_has_no_user_home_or_tmp_paths_and_checks_failed_kernels(self):
        patch_path = SOURCE / 'contrib/frixit/drivers/update-hook.patch'
        content = patch_path.read_text(encoding='utf-8')
        # Inspect post-patch lines (context + added lines, ignoring removed '-' lines and diff headers).
        effective_lines = [
            line[1:] if line.startswith(('+', ' ')) else line
            for line in content.splitlines()
            if not line.startswith(('---', '+++', '@@', '-'))
        ]
        effective_text = '\n'.join(effective_lines)

        self.assertNotRegex(
            effective_text,
            r'/home/[^\s"\']+',
            'N03: root-run pacman hook/worker must not execute scripts or patches from a user /home directory',
        )
        self.assertNotRegex(
            effective_text,
            r'LOG_FILE\s*=\s*["\']?/tmp/',
            'N03: root-run pacman hook must not write logs to a predictable /tmp path',
        )

        # .installed_version marker must only be written AFTER verifying FAILED_KERNELS is empty.
        version_idx = next(
            (i for i, line in enumerate(effective_lines) if '.installed_version' in line),
            None,
        )
        failed_check_idx = next(
            (i for i, line in enumerate(effective_lines) if 'if [ -n "$FAILED_KERNELS" ]' in line),
            None,
        )
        self.assertIsNotNone(failed_check_idx, 'N03: expected FAILED_KERNELS guard in worker patch')
        if version_idx is not None:
            self.assertGreater(
                version_idx,
                failed_check_idx,
                'N03: .installed_version is written before checking FAILED_KERNELS',
            )

    def test_n27_deploy_system_copies_package_to_private_dir_and_blocks_path_traversal(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)

            # 1) Private copy check: pacman -U must receive a private root-owned copy
            #    of the verified package, not the user-writable bundle path.
            root = base / 'root'
            bundle = base / 'bundle'
            bundle.mkdir()
            src_cfg = root / 'home/test/Data/config.json'
            src_cfg.parent.mkdir(parents=True)
            src_cfg.write_text('{"original":true}')
            pkg = bundle / 'frixit-control-center-0.2.0-1-x86_64.pkg.tar.zst'
            pkg.write_bytes(b'fake-package-bytes')
            manifest = {
                'package': pkg.name,
                'packageSha256': deploy.digest(pkg),
                'inputs': [{'path': '/home/test/Data/config.json', 'sha256': deploy.digest(src_cfg)}],
            }
            (bundle / 'deployment.json').write_text(json.dumps(manifest))

            system = FakeSystem(root)
            deployment = deploy.Deployment(bundle, root, system)
            deployment.install()

            pacman_u_calls = [c for c in system.calls if c[:2] == ('pacman', '-U')]
            self.assertEqual(len(pacman_u_calls), 1)
            installed_pkg_arg = Path(pacman_u_calls[0][-1]).resolve()
            self.assertNotEqual(
                installed_pkg_arg.parent,
                bundle.resolve(),
                'N27: pacman -U must install from a private root-owned copy, not directly from the user bundle dir',
            )

            # 2) Path traversal check: ../ components in manifest inputs must be rejected.
            root2 = base / 'root2'
            bundle2 = base / 'bundle2'
            bundle2.mkdir()
            secret = root2 / 'etc/secret.conf'
            secret.parent.mkdir(parents=True)
            secret.write_text('secret-data')
            (root2 / 'home/test').mkdir(parents=True)
            pkg2 = bundle2 / 'frixit-control-center-0.2.0-1-x86_64.pkg.tar.zst'
            pkg2.write_bytes(b'fake-package-bytes')
            traversal_manifest = {
                'package': pkg2.name,
                'packageSha256': deploy.digest(pkg2),
                'inputs': [{'path': '/home/test/../../etc/secret.conf', 'sha256': deploy.digest(secret)}],
            }
            (bundle2 / 'deployment.json').write_text(json.dumps(traversal_manifest))
            with self.assertRaises(
                ValueError,
                msg='N27: deploy-system.py must reject ../ path traversal in manifest inputs',
            ):
                deploy.Deployment(bundle2, root2, FakeSystem(root2)).verify_bundle()

            # 3) Symlink check: symlinked manifest input must be rejected.
            root3 = base / 'root3'
            bundle3 = base / 'bundle3'
            bundle3.mkdir()
            outside = base / 'outside_target.json'
            outside.write_text('{"outside":true}')
            link_input = root3 / 'home/test/Data/config.json'
            link_input.parent.mkdir(parents=True)
            link_input.symlink_to(outside)
            pkg3 = bundle3 / 'frixit-control-center-0.2.0-1-x86_64.pkg.tar.zst'
            pkg3.write_bytes(b'fake-package-bytes')
            symlink_manifest = {
                'package': pkg3.name,
                'packageSha256': deploy.digest(pkg3),
                'inputs': [{'path': '/home/test/Data/config.json', 'sha256': deploy.digest(outside)}],
            }
            (bundle3 / 'deployment.json').write_text(json.dumps(symlink_manifest))
            with self.assertRaises(
                ValueError,
                msg='N27: deploy-system.py must reject symlinked manifest inputs',
            ):
                deploy.Deployment(bundle3, root3, FakeSystem(root3)).verify_bundle()

    def test_r19_run_rust_interop_preserves_orig_home_and_toolchain(self):
        script = SOURCE / 'tests/run_rust_interop_test.sh'
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            orig_home = base / 'orig_home'
            cargo_bin_dir = orig_home / '.cargo/bin'
            cargo_bin_dir.mkdir(parents=True)
            fake_source = base / 'fake_repo'
            fake_source.mkdir()
            (fake_source / 'Cargo.toml').write_text('[workspace]\n')
            toolchain_log = base / 'toolchain.log'
            daemon_out = fake_source / 'target/debug/examples/interop_daemon'

            # Fake cargo in $ORIG_HOME/.cargo/bin that records RUSTUP_TOOLCHAIN and creates interop_daemon.
            fake_cargo = cargo_bin_dir / 'cargo'
            fake_cargo.write_text(
                f'#!/usr/bin/env bash\n'
                f'printf "%s" "${{RUSTUP_TOOLCHAIN:-<unset>}}" > "{toolchain_log}"\n'
                f'mkdir -p "{daemon_out.parent}"\n'
                f'printf "#!/usr/bin/env bash\\nexit 0\\n" > "{daemon_out}"\n'
                f'chmod +x "{daemon_out}"\n'
            )
            fake_cargo.chmod(fake_cargo.stat().st_mode | stat.S_IXUSR)

            fake_test_bin = base / 'fake_test_bin'
            fake_test_bin.write_text('#!/usr/bin/env bash\nexit 0\n')
            fake_test_bin.chmod(fake_test_bin.stat().st_mode | stat.S_IXUSR)

            env = {
                'HOME': str(orig_home),
                'PATH': '/usr/bin:/bin',
                'DBUS_SESSION_BUS_ADDRESS': 'unix:path=/tmp/fake-ucc-bus',
            }
            proc = subprocess.run(
                ['bash', str(script), str(fake_test_bin), str(fake_source)],
                env=env,
                capture_output=True,
                text=True,
                check=False,
            )
            self.assertEqual(
                proc.returncode,
                0,
                f'R19: run_rust_interop_test.sh failed to find cargo from original HOME/.cargo/bin:\n'
                f'stdout={proc.stdout}\nstderr={proc.stderr}',
            )
            self.assertTrue(toolchain_log.is_file(), 'R19: fake cargo in original HOME was never invoked')
            recorded_toolchain = toolchain_log.read_text(encoding='utf-8').strip()
            self.assertNotEqual(
                recorded_toolchain,
                'stable',
                'R19: run_rust_interop_test.sh forced RUSTUP_TOOLCHAIN=stable instead of respecting rust-toolchain.toml',
            )

    def test_r18_sysfs_path_containment_does_not_silently_skip(self):
        rs_path = SOURCE / 'crates/ucc-hardware/tests/sysfs_path_containment.rs'
        content = rs_path.read_text(encoding='utf-8')
        # A silent `if !shim_active { ... return; }` makes `cargo test` report `ok`
        # without ever exercising the symlink-swap race assertion.
        silent_skip = re.search(
            r'if\s+!shim_active\s*\{[^}]*\breturn\s*;',
            content,
            flags=re.DOTALL,
        )
        self.assertIsNone(
            silent_skip,
            'R18: sysfs_path_containment.rs silently returns ok when LD_PRELOAD shim is inactive',
        )


    def test_n22_plasma_tray_handles_json_object_keyboard_profile(self):
        tray_cpp = SOURCE / 'ucc-tray/src/TrayBackend.cpp'
        content = tray_cpp.read_text(encoding='utf-8')
        match = re.search(
            r'void\s+TrayBackend::setActiveKeyboardProfile\s*\([^)]*\)\s*\{(.*?)\n\}',
            content,
            flags=re.DOTALL,
        )
        self.assertIsNotNone(match, 'N22: could not locate TrayBackend::setActiveKeyboardProfile in TrayBackend.cpp')
        body = match.group(1)

        self.assertNotRegex(
            body,
            r'\[\s*"json"\s*\]\s*\.toString\s*\(\s*\)',
            'N22: TrayBackend::setActiveKeyboardProfile unconditionally calls ["json"].toString(), '
            'which returns empty when "json" is a JSON object instead of a string',
        )
        handles_json_value_object = bool(re.search(r'(?<!\bdoc)\.isObject\s*\(\s*\)', body))
        self.assertTrue(
            handles_json_value_object,
            'N22: TrayBackend::setActiveKeyboardProfile must check QJsonValue::isObject() on the "json" field',
        )

    def test_n32_preview_ctest_separates_write_expecting_targets(self):
        cmake_path = SOURCE / 'tests/CMakeLists.txt'
        lines = cmake_path.read_text(encoding='utf-8').splitlines()

        write_targets = (
            'test_gui_rust_e2e',
            'test_system_monitor',
            'test_rust_uccd_interop',
        )
        gated = {}
        cond_stack = []
        for idx, raw_line in enumerate(lines, start=1):
            line = raw_line.split('#', 1)[0].strip()
            if not line:
                continue
            if_match = re.match(r'^if\s*\((.*)\)\s*$', line, flags=re.IGNORECASE)
            if if_match:
                cond_stack.append(if_match.group(1))
                continue
            elif_match = re.match(r'^elseif\s*\((.*)\)\s*$', line, flags=re.IGNORECASE)
            if elif_match and cond_stack:
                cond_stack[-1] = elif_match.group(1)
                continue
            if re.match(r'^else\s*\(\s*\)\s*$', line, flags=re.IGNORECASE) and cond_stack:
                cond_stack[-1] = f'NOT ({cond_stack[-1]})'
                continue
            if re.match(r'^endif\s*\(.*\)\s*$', line, flags=re.IGNORECASE) and cond_stack:
                cond_stack.pop()
                continue

            for target in write_targets:
                if re.search(rf'\badd_test\s*\(.*\b{re.escape(target)}\b', line) or (
                    re.search(rf'\bNAME\s+{re.escape(target)}\b', line)
                ):
                    is_preview_gated = any(
                        re.search(r'\bNOT\s+UCC_READ_ONLY_PREVIEW\b', c)
                        for c in cond_stack
                    )
                    gated[target] = (idx, is_preview_gated)

        for target in write_targets:
            self.assertIn(target, gated, f'N32: expected CTest registration for {target} in tests/CMakeLists.txt')
            line_no, is_gated = gated[target]
            self.assertTrue(
                is_gated,
                f'N32: write-expecting CTest target {target} (tests/CMakeLists.txt:{line_no}) '
                f'is not gated under if(NOT UCC_READ_ONLY_PREVIEW)',
            )

    def test_m_ext_10_cmake_build_tray_defaults_off_per_adr03(self):
        cmake_path = SOURCE / 'CMakeLists.txt'
        content = cmake_path.read_text(encoding='utf-8')
        match = re.search(
            r'option\s*\(\s*BUILD_TRAY\s+"[^"]*"\s+(ON|OFF)\s*\)',
            content,
            flags=re.IGNORECASE,
        )
        self.assertIsNotNone(match, 'M-EXT-10: BUILD_TRAY option not found in root CMakeLists.txt')
        self.assertEqual(
            match.group(1).upper(),
            'OFF',
            f'M-EXT-10: CMakeLists.txt sets BUILD_TRAY default to {match.group(1)} instead of OFF per ADR-03',
        )

    def test_m_pkg_06_ci_workflows_have_permissions_and_pinned_actions(self):
        workflows_dir = SOURCE / '.github/workflows'
        workflow_files = sorted(
            list(workflows_dir.glob('*.yml')) + list(workflows_dir.glob('*.yaml'))
        )
        self.assertTrue(workflow_files, 'M-PKG-06: expected workflow files under .github/workflows/')

        missing_permissions = []
        unpinned_actions = []
        for wf in workflow_files:
            content = wf.read_text(encoding='utf-8')
            if not re.search(r'(?m)^\s*permissions\s*:', content):
                missing_permissions.append(wf.name)
            for ref in re.findall(r'(?m)^\s*-?\s*uses:\s*([^\s#]+@(?:master|main))\b', content):
                unpinned_actions.append(f'{wf.name}: {ref}')

        self.assertFalse(
            missing_permissions,
            f'M-PKG-06: CI workflows missing explicit permissions block: {missing_permissions}',
        )
        self.assertFalse(
            unpinned_actions,
            f'M-PKG-06: CI workflows use unpinned @master/@main actions: {unpinned_actions}',
        )



class M13GuiLddAndPackagingInvariantsTest(unittest.TestCase):
    CLEAN_LDD_OUTPUT = (
        '\tlinux-vdso.so.1 (0x00007fff12340000)\n'
        '\tlibQt6Charts.so.6 => /usr/lib/libQt6Charts.so.6 (0x00007f0000010000)\n'
        '\tlibQt6Widgets.so.6 => /usr/lib/libQt6Widgets.so.6 (0x00007f0000020000)\n'
        '\tlibQt6Gui.so.6 => /usr/lib/libQt6Gui.so.6 (0x00007f0000030000)\n'
        '\tlibQt6DBus.so.6 => /usr/lib/libQt6DBus.so.6 (0x00007f0000040000)\n'
        '\tlibQt6Core.so.6 => /usr/lib/libQt6Core.so.6 (0x00007f0000050000)\n'
        '\tlibxcb.so.1 => /usr/lib/libxcb.so.1 (0x00007f0000060000)\n'
        '\tlibc.so.6 => /usr/lib/libc.so.6 (0x00007f0000070000)\n'
    )

    def _run_ldd_check(self, args=(), env_extra=None):
        script = SOURCE / 'scripts/check_gui_ldd.sh'
        env = {'PATH': os.environ.get('PATH', '/usr/bin:/bin')}
        if env_extra:
            env.update(env_extra)
        return subprocess.run(
            [str(script), *args],
            env=env,
            capture_output=True,
            text=True,
            check=False,
        )

    def test_m12_7_check_gui_ldd_script_properties_and_arg_handling(self):
        script = SOURCE / 'scripts/check_gui_ldd.sh'
        self.assertTrue(script.is_file(), 'scripts/check_gui_ldd.sh must exist')
        mode = script.stat().st_mode
        self.assertTrue(mode & stat.S_IXUSR, 'scripts/check_gui_ldd.sh must be executable (+x)')
        content = script.read_text(encoding='utf-8')
        self.assertIn('set -euo pipefail', content)

        help_proc = self._run_ldd_check(['--help'])
        self.assertEqual(help_proc.returncode, 0, help_proc.stderr)
        self.assertIn('Usage:', help_proc.stdout)

        missing_arg = self._run_ldd_check()
        self.assertEqual(missing_arg.returncode, 2)

        unknown_opt = self._run_ldd_check(['--bogus'])
        self.assertEqual(unknown_opt.returncode, 2)

        missing_bin = self._run_ldd_check(['/nonexistent/path/to/ucc-gui'])
        self.assertEqual(missing_bin.returncode, 2)

        empty_out = self._run_ldd_check(env_extra={'UCC_LDD_OUTPUT': '   \n'})
        self.assertNotEqual(empty_out.returncode, 0)

    def test_m12_7_check_gui_ldd_accepts_clean_output_and_real_binary(self):
        ok_env = self._run_ldd_check(['ucc-gui'], env_extra={'UCC_LDD_OUTPUT': self.CLEAN_LDD_OUTPUT})
        self.assertEqual(ok_env.returncode, 0, ok_env.stderr)
        self.assertIn('OK:', ok_env.stdout)

        with tempfile.TemporaryDirectory() as tmp:
            mock_file = Path(tmp) / 'ldd.txt'
            mock_file.write_text(self.CLEAN_LDD_OUTPUT, encoding='utf-8')
            ok_file = self._run_ldd_check(['ucc-gui'], env_extra={'UCC_LDD_OUTPUT_FILE': str(mock_file)})
            self.assertEqual(ok_file.returncode, 0, ok_file.stderr)

            fake_bin = Path(tmp) / 'ucc-gui'
            fake_bin.write_text('dummy', encoding='utf-8')
            fake_ldd = Path(tmp) / 'fake-ldd'
            fake_ldd.write_text(
                f'#!/usr/bin/env bash\nprintf "%s" {json.dumps(self.CLEAN_LDD_OUTPUT)}\n',
                encoding='utf-8',
            )
            fake_ldd.chmod(fake_ldd.stat().st_mode | stat.S_IXUSR)
            ok_ldd_bin = self._run_ldd_check(
                [str(fake_bin)],
                env_extra={'UCC_LDD_BIN': str(fake_ldd)},
            )
            self.assertEqual(ok_ldd_bin.returncode, 0, ok_ldd_bin.stderr)

        real_bin = Path('/bin/true')
        if real_bin.is_file():
            ok_real = self._run_ldd_check([str(real_bin)])
            self.assertEqual(ok_real.returncode, 0, ok_real.stderr)

    def test_m12_7_check_gui_ldd_rejects_all_forbidden_dependencies(self):
        forbidden_cases = [
            ('not found', '\tlibmissing.so.1 => not found\n'),
            ('KF6', '\tlibKF6WindowSystem.so.6 => /usr/lib/libKF6WindowSystem.so.6 (0x1)\n'),
            ('KF6', '\tlibKF6StatusNotifierItem.so.6 => /usr/lib/libKF6StatusNotifierItem.so.6 (0x1)\n'),
            ('Qt6Qml', '\tlibQt6Qml.so.6 => /usr/lib/libQt6Qml.so.6 (0x1)\n'),
            ('Qt6Quick', '\tlibQt6Quick.so.6 => /usr/lib/libQt6Quick.so.6 (0x1)\n'),
            ('Qt6Bluetooth', '\tlibQt6Bluetooth.so.6 => /usr/lib/libQt6Bluetooth.so.6 (0x1)\n'),
            ('libucc-dbus.so', '\tlibucc-dbus.so.0 => /usr/lib/libucc-dbus.so.0 (0x1)\n'),
            ('libucc-dbus.so', '\tlibucc-dbus-preview.so => /tmp/libucc-dbus-preview.so (0x1)\n'),
        ]
        for label, bad_line in forbidden_cases:
            with self.subTest(forbidden=label, line=bad_line.strip()):
                proc = self._run_ldd_check(
                    ['ucc-gui'],
                    env_extra={'UCC_LDD_OUTPUT': self.CLEAN_LDD_OUTPUT + bad_line},
                )
                self.assertNotEqual(
                    proc.returncode,
                    0,
                    f'check_gui_ldd.sh unexpectedly passed with forbidden {label}: {bad_line}',
                )
                self.assertIn(f'FAIL [{label}]', proc.stderr)

    def test_m12_7_cmake_and_ci_wire_gui_ldd_invariants_without_kf6_leak(self):
        top_cmake_raw = (SOURCE / 'CMakeLists.txt').read_text(encoding='utf-8')
        top_cmake = '\n'.join(
            line.split('#', 1)[0] for line in top_cmake_raw.splitlines()
        )
        self.assertNotIn(
            'KF6WindowSystem',
            top_cmake,
            'ADR-01/M12.7: root CMakeLists.txt must not link KF6WindowSystem into ucc-gui in default or M13 builds',
        )
        self.assertNotIn(
            'KF6::WindowSystem',
            top_cmake,
            'ADR-01/M12.7: root CMakeLists.txt must not link KF6::WindowSystem into ucc-gui',
        )
        self.assertNotRegex(
            top_cmake,
            r'if\s*\([^)]*BUILD_GUI[^)]*\)\s*\n\s*find_package\s*\(\s*KF6StatusNotifierItem',
            'ucc-native-tray KF6StatusNotifierItem lookup must not be triggered by BUILD_GUI',
        )

        tests_cmake = (SOURCE / 'tests/CMakeLists.txt').read_text(encoding='utf-8')
        self.assertIn('test_gui_ldd_invariants', tests_cmake)
        self.assertIn('scripts/check_gui_ldd.sh', tests_cmake)
        self.assertIn('$<TARGET_FILE:ucc-gui>', tests_cmake)

        build_yml = (SOURCE / '.github/workflows/build.yml').read_text(encoding='utf-8')
        for expected_call in (
            'scripts/check_gui_ldd.sh build/bin/ucc-gui',
            'scripts/check_gui_ldd.sh build-rust-runtime/bin/ucc-gui',
            'scripts/check_gui_ldd.sh build-preview/bin/ucc-gui',
        ):
            self.assertIn(
                expected_call,
                build_yml,
                f'build.yml must invoke {expected_call} to enforce M12.7/ADR-01 invariants',
            )

    def test_m13_packaging_manifests_free_of_kf6_qml_bluetooth_for_gui(self):
        # PKGBUILD runtime depends must not pull KF6, QML, or Qt6Bluetooth.
        pkgbuild = (SOURCE / 'PKGBUILD').read_text(encoding='utf-8')
        depends_match = re.search(r'(?m)^depends=\(([^)]*)\)', pkgbuild)
        self.assertIsNotNone(depends_match, 'PKGBUILD must declare depends=(...)')
        pkgbuild_depends = depends_match.group(1)
        for forbidden in ('kf6', 'kwindowsystem', 'kstatusnotifieritem', 'plasma', 'qt6-declarative', 'qt6-connectivity'):
            self.assertNotIn(
                forbidden,
                pkgbuild_depends.lower(),
                f'PKGBUILD depends must not include {forbidden}',
            )

        # debian/control must not pull KF6, QML, or Qt6Bluetooth.
        deb_control = (SOURCE / 'debian/control').read_text(encoding='utf-8').lower()
        for forbidden in ('libkf6', 'kf6-', 'qt6-declarative', 'qt6-connectivity', 'libplasma'):
            self.assertNotIn(
                forbidden,
                deb_control,
                f'debian/control must not depend on {forbidden}',
            )

        # ucc.spec must not pull KF6, QML, or Qt6Bluetooth.
        spec = (SOURCE / 'ucc.spec').read_text(encoding='utf-8').lower()
        for forbidden in ('kf6-', 'qt6-qtdeclarative', 'qt6-qtconnectivity', 'libplasma'):
            self.assertNotIn(
                forbidden,
                spec,
                f'ucc.spec must not depend on {forbidden}',
            )

        # package.nix must not pull Plasma/Breeze/KF6/QML/Qt6Bluetooth into buildInputs.
        package_nix = (SOURCE / 'package.nix').read_text(encoding='utf-8').lower()
        for forbidden in ('plasma-integration', 'breeze', 'kwindowsystem', 'kstatusnotifieritem', 'qtdeclarative', 'qtconnectivity'):
            self.assertNotIn(
                forbidden,
                package_nix,
                f'package.nix must not pull {forbidden} into ucc closure',
            )

        # flake.nix devShell must not pull retired extra-cmake-modules.
        flake_nix = (SOURCE / 'flake.nix').read_text(encoding='utf-8').lower()
        self.assertNotIn(
            'extra-cmake-modules',
            flake_nix,
            'flake.nix devShell must not require retired KDE extra-cmake-modules',
        )

if __name__ == '__main__':
    unittest.main()
