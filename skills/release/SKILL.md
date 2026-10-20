---
name: release
description: Prepare an io-to-net release — dependencies and audit, tests on macOS and Linux, version and changelog, firmware builds, packages (deb/rpm/zip/tar.gz), GitHub release, Homebrew.
---

# Release

1. `cargo update`; check direct dependencies for new majors
   (`cargo search <crate> --limit 1`) and raise the minimums in `Cargo.toml`.
   `cargo audit` must be clean. Fix by updating; exceptions go only in
   `.cargo/audit.toml`, with a reason.
2. `cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test`,
   then the same with `--no-default-features`. Repeat on Linux in Docker (see
   AGENTS.md).
3. Bump `version` in `Cargo.toml` (and `FIRMWARE_VERSION` in
   `firmware/platformio.ini` if the firmware changed). Add a `CHANGELOG.md`
   entry: breaking changes first, then added, fixed, changed.
4. Firmware: pin the newest `espressif32` / `espressif8266` platforms and
   build every environment with all bridges enabled. Build in a copy with
   `secrets.h` taken from the example; never commit `secrets.h`. On Apple
   Silicon, build the ESP8266 environment in an arm64 container (python:3.12
   + platformio).
5. Hardware check (hardware-validation skill), or say it was skipped.
6. `./scripts/build_packages.sh [--platform …]` writes to `release/<version>/`
   (gitignored), using `cross` and Docker; `Cross.toml` installs libdbus.
   Inspect the packages: they ship `LICENCE` and `AI_TRAINING_RESTRICTION.md`.
7. GitHub release `<version>` with the packages and SHA256SUMS. The maintainer
   runs `crowfoundry/codebase/infrastructure/infra/deploy/scripts/publish-packages.sh
   --repo askrejans/io-to-net --tag <v>`.
8. Homebrew formula `io-to-net.rb` in `../homebrew/homebrew-g86racing`
   (`scripts/bump-formula.sh`), which is a separate repository.

Ask before pushing, tagging or publishing. Do not set
`CARGO_CACHE_AUTO_CLEAN_FREQUENCY`.
