# SPDX-License-Identifier: GPL-2.0-only
# Out-of-tree and DKMS Makefile for uwcc-rs (Linux 7.3+, CONFIG_RUST=y).

KVER ?= $(shell uname -r)
KDIR ?= /lib/modules/$(KVER)/build
PWD  := $(shell pwd)

PACKAGE_NAME    := uwcc-rs
PACKAGE_VERSION := 0.0.1
DKMS_SRC_DIR    := /usr/src/$(PACKAGE_NAME)-$(PACKAGE_VERSION)

.PHONY: all modules clean install modules_install check-rust dkms-install dkms-remove

all: modules

check-rust:
	@if [ ! -d "$(KDIR)" ]; then \
		echo "ERROR: Kernel build directory not found: $(KDIR)" >&2; \
		exit 1; \
	fi
	@if [ -f "$(KDIR)/.config" ] && ! grep -q '^CONFIG_RUST=y' "$(KDIR)/.config"; then \
		echo "ERROR: Target kernel $(KDIR) does not have CONFIG_RUST=y enabled." >&2; \
		exit 1; \
	fi

modules: check-rust
	$(MAKE) -C "$(KDIR)" M="$(PWD)" modules

install: modules_install

modules_install:
	$(MAKE) -C "$(KDIR)" M="$(PWD)" modules_install
	depmod -a "$(KVER)"

clean:
	$(MAKE) -C "$(KDIR)" M="$(PWD)" clean

dkms-install:
	install -d "$(DKMS_SRC_DIR)"
	cp -a Kbuild Makefile dkms.conf include src "$(DKMS_SRC_DIR)/"
	dkms add -m "$(PACKAGE_NAME)" -v "$(PACKAGE_VERSION)"
	dkms build -m "$(PACKAGE_NAME)" -v "$(PACKAGE_VERSION)" -k "$(KVER)"
	dkms install -m "$(PACKAGE_NAME)" -v "$(PACKAGE_VERSION)" -k "$(KVER)"

dkms-remove:
	dkms remove -m "$(PACKAGE_NAME)" -v "$(PACKAGE_VERSION)" --all || true
	rm -rf "$(DKMS_SRC_DIR)"
