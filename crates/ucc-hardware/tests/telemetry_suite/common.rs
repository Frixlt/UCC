// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::Path;

pub(crate) fn write_nvidia_pci_dev(pci_root: &Path, bdf: &str, status: &str, control: &str) {
    let base = pci_root.join(bdf);
    std::fs::create_dir_all(base.join("power")).unwrap();
    std::fs::write(base.join("vendor"), "0x10de").unwrap();
    std::fs::write(base.join("class"), "0x030000").unwrap();
    std::fs::write(base.join("power/runtime_status"), status).unwrap();
    std::fs::write(base.join("power/control"), control).unwrap();
}
