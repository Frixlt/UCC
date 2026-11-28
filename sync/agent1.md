# Agent 1 — Kernel uwcc-rs, ucc-hardware, ucc-lighting
- **Branch:** `release/a1-kernel-hardware`
- **Status:** `PHASE1_DONE`
- **Base SHA:** `e1fe413`
- **HEAD SHA:** `7727da7`

## Completed
- [x] 1.1 `kernel/uwcc-rs`: allowlist cleanup (-0x0753/0x0762/0x07A4/0x07A5, +0x0746/0x074E/0x0787/0x07D0/0x07E2..0x07E5), atomic `uwcc_acpi_ec_rmw` with bitmasks (PL4D, 0x07C6 BIT(6), 0x07C5 BIT(4)), PWUP <= 7, ec_version 0x0454/0x047D, SMRW fix, DKMS/udev/SPDX
- [x] 1.2 `uwcc/{mod,ramfan}.rs`: real 96-byte RamFan 1.5, PL4 x2, SMRW data[4]/data[7] fix, real TCC 0x0786/0x0463, GPU 0x0743..0x0746 + live 0x0788/0x07D5, read_pmsc_mode, Fn Lock 0x074E, 0x07D0, 0x07A6 0x30, 0x0748 BIT(2), tear-free RPM, Drop cleanup
- [x] 1.3 `detection/platform/wmi_events/uapi/webcam`: Uniwill before Clevo, X6AR vs X6FR disambiguation, frozen PlatformBackend methods, upstream uniwill-wmi.h codes, UAPI size_of asserts, PWUP <= 7
- [x] 1.4 non-uwcc hardware: P0 water-cooler Manual overheat failsafe (90/80 C), SysfsRoot O_NOFOLLOW + CPU rollback, non-blocking FanController, BlueZ timeouts, poison-safe locks
- [x] 1.5 `ucc-lighting`: exclude 0xFF89 (0xFF03>0xFF12>0xFF02), 048D:7001/7000/CE00 protocols, unified feature framing, 30 Hz animation, KeyboardBacklightController::on_resume()
- [ ] Phase 2: 5 independent adversarial reviewers (launching now)

## Tests & Verification
- `kernel/uwcc-rs/src/validation.rs`: PASS
- `cargo fmt --all -- --check`: PASS
- `RUSTFLAGS="-D warnings" dbus-run-session -- cargo test -p ucc-hardware -p ucc-lighting`: 100% PASS
- `cargo check -p uccd`: PASS

## Cross-Agent Notes
- All frozen CONTRACT.md signatures (`read_dynamic_boost_watts`, `set_dynamic_boost_watts`, `read_pmsc_mode`, `read_tcc_offset`, `set_tcc_offset`) are implemented and verified.
