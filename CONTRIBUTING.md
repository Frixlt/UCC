# Contributing to `ucc-rusted`

Technical workflow, coding standards, verification gates, and release procedures for `ucc-rusted`.

## Branching & Commit Model

- **Trunk-based development**: `main` is the integration branch; short-lived topic branches (`feat/...`, `fix/...`, `docs/...`) merge into `main` via fast-forward or clean rebase.
- **Atomic commits**: each completed logical change (feature, bugfix, refactoring step, test suite, or documentation update) is committed immediately after passing checks. Do not commit build artifacts, ELF/shared libraries, or temporary fixture files.
- **Conventional Commits**: all commit headers MUST follow `<type>(<scope>): <imperative summary>` (or `<type>!: <summary>` for breaking changes).
  - Allowed types: `feat`, `fix`, `perf`, `refactor`, `docs`, `test`, `build`, `ci`, `chore`.
  - Standard scopes: `uccd`, `hardware`, `lighting`, `dbus`, `cli`, `desktop`, `session`, `gui`, `xtask`, `pkg`, `docs`, `ci`, `plan`.
  - Example: `fix(uccd): restore aquaris.json RGB target on resume reconnect`

## Code Style & Static Analysis

- **Editor defaults**: `.editorconfig` enforces UTF-8, LF line endings, and language-specific indentation.
- **Rust** (`crates/*`, MSRV `1.89`, edition `2021`):
  - Formatting rules in `rustfmt.toml` (`max_width = 100`, Unix LF). Run `cargo fmt --check` when `rustfmt` is installed.
  - Zero warnings allowed under `cargo clippy --workspace --all-targets --locked -- -D warnings`.
- **C++20 / Qt 6** (`ucc-gui/`, `ucc-desktop/`, `ucc-session/`, `tests/`):
  - Formatting rules in `.clang-format` (`BasedOnStyle: LLVM`, 2-space indent, 120-column limit, Allman braces).
  - Compiler flags `-Wall -Wextra -Wpedantic`; standalone GUI builds (`-DUCC_PACKAGE_RUST_RUNTIME=ON`) MUST remain free of `KF6`, `Qt6Qml`, `Qt6Quick`, `Qt6Bluetooth`, and `libucc-dbus`.

## Verification Gates

Run before committing or tagging a release:

```bash
cargo clippy --workspace --all-targets --locked --offline -- -D warnings
dbus-run-session -- cargo test --workspace --locked --offline --no-fail-fast
cargo run -p xtask --locked --offline -- check-versions
```

For C++ GUI/integration builds:

```bash
cmake -S . -B /tmp/ucc-build -G Ninja \
  -DCMAKE_BUILD_TYPE=RelWithDebInfo \
  -DUCC_PACKAGE_RUST_RUNTIME=ON \
  -DBUILD_TRAY=OFF \
  -DBUILD_TESTS=ON
cmake --build /tmp/ucc-build
ctest --test-dir /tmp/ucc-build --output-on-failure
```

## Versioning & Release Procedure (SemVer + `xtask`)

Versioning follows **Semantic Versioning 2.0.0**:

- **`0.x.y`**: active pre-1.0 series while hardware acceptance across external coolers and vendor models proceeds.
- **`1.0.0+`**: promoted once hardware, compositor, and distribution acceptance criteria are satisfied.
- Six repository manifests share the authoritative version string:
  `Cargo.toml`, `CMakeLists.txt`, `PKGBUILD`, `ucc.spec`, `package.nix`, and `debian/changelog`.

### Preparing a Release

1. Verify current manifest parity:
   ```bash
   cargo run -p xtask --locked --offline -- check-versions
   ```
2. Preview version updates without writing files:
   ```bash
   cargo run -p xtask --locked --offline -- release <X.Y.Z> --dry-run
   ```
3. Apply version bump (and update `CHANGELOG.md` automatically if `git-cliff` is available on `PATH`):
   ```bash
   cargo run -p xtask --locked --offline -- release <X.Y.Z>
   ```
4. Refresh `Cargo.lock` workspace version entries if needed, run full Rust + CMake/CTest verification, commit and tag:
   ```bash
   cargo update --workspace --offline
   git add Cargo.toml Cargo.lock CMakeLists.txt PKGBUILD ucc.spec package.nix debian/changelog CHANGELOG.md
   git commit -m "chore(release): v<X.Y.Z>"
   git tag -a v<X.Y.Z> -m "v<X.Y.Z>"
   ```
