.PHONY: all dist clean distclean print-name help build rpm srpm deb arch nix

# Extract version from CMakeLists.txt (preferred method)
VERSION := $(shell grep -E 'VERSION\s+[0-9]+\.[0-9]+\.[0-9]+' CMakeLists.txt | head -1 | grep -oE '[0-9]+\.[0-9]+\.[0-9]+')

# If VERSION is empty, try git tag
ifeq ($(VERSION),)
  VERSION := $(shell git describe --tags --abbrev=0 2>/dev/null | sed 's/^v//')
endif

# If VERSION is still empty, use 0.2.0 as fallback
ifeq ($(VERSION),)
  VERSION := 0.2.0
endif

# Get git hash for archive naming
GITHASH := $(shell git rev-parse --short HEAD 2>/dev/null || echo unknown)

# Release timestamp: 0.YYYYMMDD.HHMM
RELEASE_TS := 0.$(shell date +%Y%m%d.%H%M)

# Archive name: ucc-<version>-<githash>.tar.gz
DISTNAME := ucc-$(VERSION)-$(GITHASH)
ARCHIVE := $(DISTNAME).tar.gz

# Temporary directory used when creating the dist tarball
TMPDIR := dist/$(DISTNAME)-tmp

help:
	@echo "Uniwill Control Center - Build and Package Targets"
	@echo ""
	@echo "Build targets:"
	@echo "  make build              Build the project locally"
	@echo "  make clean              Clean build artifacts"
	@echo ""
	@echo "Package targets:"
	@echo "  make dist               Create source tarball"
	@echo "  make rpm                Create RPM packages (Fedora/RHEL)"
	@echo "  make srpm               Create SRPM only (Fedora/RHEL)"
	@echo "  make deb                Create Debian packages (Debian/Ubuntu)"
	@echo "  make arch               Create Arch Linux package"
	@echo "  make nix                Create Nix package"
	@echo ""
	@echo "Other:"
	@echo "  make help               Show this help message"
	@echo "  make print-name         Print archive name"

all: build

build:
	cargo build --workspace --bins --release --locked --target-dir target
	cmake -S . -B build -G Ninja -DCMAKE_BUILD_TYPE=Release \
		-DCMAKE_INSTALL_PREFIX=/usr -DCMAKE_INSTALL_LIBDIR=lib \
		-DBUILD_TRAY=OFF -DBUILD_CLI=OFF -DUCC_PACKAGE_RUST_RUNTIME=ON
	cmake --build build --parallel 2

clean:
	@echo "Cleaning build artifacts..."
	@rm -rf build dist
	@find . -name "CMakeCache.txt" -delete
	@find . -name "CMakeFiles" -type d -exec rm -rf {} + 2>/dev/null || true
	@echo "Clean complete"

distclean: clean
	@echo "Removing distribution artifacts..."
	@rm -f $(ARCHIVE) ucc-*.tar.gz
	@rm -rf dist
	@echo "Distclean complete"

dist:
	@echo "Creating source archive: $(ARCHIVE)"
	@mkdir -p dist
	@rm -rf $(TMPDIR); mkdir -p $(TMPDIR)
	@echo "Preparing source tree in $(TMPDIR)/ucc-$(VERSION)..."
	@git archive --format=tar --prefix=ucc-$(VERSION)/ --output=$(TMPDIR)/source.tar HEAD
	@tar xf $(TMPDIR)/source.tar -C $(TMPDIR)
	@printf '%s\n' "$(GITHASH)" > $(TMPDIR)/ucc-$(VERSION)/.release-commit
	@bash contrib/package-rust-vendor.sh $(TMPDIR)/ucc-$(VERSION)
	@tar czf dist/$(ARCHIVE) -C $(TMPDIR) ucc-$(VERSION)
	@rm -rf $(TMPDIR)
	@echo "Created dist/$(ARCHIVE)"
	@cd dist && sha256sum $(ARCHIVE) > $(ARCHIVE).sha256
	@echo "SHA256: $$(cat dist/$(ARCHIVE).sha256)"

print-name:
	@echo $(DISTNAME)

rpm: dist
	@echo "Building RPM packages..."
	@mkdir -p $(addprefix dist/rpmbuild/,BUILD BUILDROOT RPMS SOURCES SPECS SRPMS)
	@cp dist/$(ARCHIVE) dist/rpmbuild/SOURCES/
	@# Create modified spec file with correct version, release, and source filename
	@sed -e 's/^Version:.*/Version:        $(VERSION)/' \
	     -e 's/^Release:.*/Release:        $(RELEASE_TS)/' \
	     -e 's/Source0:.*/Source0:        $(ARCHIVE)/' \
	     ucc.spec > dist/rpmbuild/SPECS/ucc.spec
	@rpmbuild -ba \
		--define "_topdir $$(pwd)/dist/rpmbuild" \
		--define "_builddir %{_topdir}/BUILD" \
		--define "_rpmdir %{_topdir}/RPMS" \
		--define "_srcrpmdir %{_topdir}/SRPMS" \
		dist/rpmbuild/SPECS/ucc.spec
	@echo "RPM packages created in dist/rpmbuild/RPMS/"
	@echo "SRPM created in dist/rpmbuild/SRPMS/"

srpm: dist
	@echo "Building SRPM package..."
	@mkdir -p $(addprefix dist/rpmbuild/,BUILD BUILDROOT RPMS SOURCES SPECS SRPMS)
	@cp dist/$(ARCHIVE) dist/rpmbuild/SOURCES/
	@# Create modified spec file with correct version, release, and source filename
	@sed -e 's/^Version:.*/Version:        $(VERSION)/' \
	     -e 's/^Release:.*/Release:        $(RELEASE_TS)/' \
	     -e 's/Source0:.*/Source0:        $(ARCHIVE)/' \
	     ucc.spec > dist/rpmbuild/SPECS/ucc.spec
	@rpmbuild -bs \
		--define "_topdir $$(pwd)/dist/rpmbuild" \
		--define "_builddir %{_topdir}/BUILD" \
		--define "_rpmdir %{_topdir}/RPMS" \
		--define "_srcrpmdir %{_topdir}/SRPMS" \
		dist/rpmbuild/SPECS/ucc.spec
	@echo "SRPM created in dist/rpmbuild/SRPMS/"

deb: dist
	@echo "Building Debian packages..."
	@mkdir -p dist/deb
	@cd dist/deb && tar xzf ../$(ARCHIVE)
	@sed -i '1s/(.*)/($(VERSION)+$(RELEASE_TS))/' dist/deb/ucc-$(VERSION)/debian/changelog
	@cd dist/deb/ucc-$(VERSION) && debuild -uc -us
	@echo "Debian packages created in dist/"

arch:
	@echo "Building Arch Linux package..."
	@mkdir -p dist/arch
	@cp PKGBUILD ucc.install dist/arch/
	@cd dist/arch && UCC_SOURCE_DIR="$(CURDIR)" makepkg -sf --noconfirm
	@echo "Arch package created in dist/arch/"

nix:
	@echo "Building Nix package..."
	@nix build .#ucc
	@echo "Nix package built (check result symlink)"
