# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/); versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `booto`, `write` and `windows` take the device as optional: leave it out to pick a removable disk from a numbered list. The typed confirmation still follows.
- `windows`: Windows 10/11 installer USB for UEFI PCs (FAT32 partition, files copied from the ISO, `install.wim` over 4 GiB split with `wimlib-imagex`).
- Runs on Windows: `list`, `write`, and `booto` (live system only; the report partition needs Linux). Disks are `\\.\PhysicalDriveN`.
- Releases ship a Linux AppImage and a Windows MSI next to the static Linux binary. CI, nightly and tagged releases build and test every package first, including an MSI install that makes a Booto USB on a virtual disk.

### Changed
- License: GPL-3.0-or-later (was MIT).

## [0.1.0] - 2026-11-05

### Added
- `list`: removable disks (`--all` for every disk).
- `booto`: downloads the newest Booto release (`--channel stable|nightly`) or uses `--iso`, checks its SHA-256, and builds the BOOTO_LIVE + BOOTO_DATA layout; `--data-size`, `--no-encrypt`.
- `write`: byte-for-byte image write with SHA-256 read-back verification; URLs need `--sha256`.
- Refuses the running system's disk, non-removable disks without `--force-nonremovable`, and asks for the device path before erasing.
- CI (fmt, clippy, test), daily `nightly-build` pre-release, tagged pre-releases, `scripts/promote.sh` for the stable channel.
