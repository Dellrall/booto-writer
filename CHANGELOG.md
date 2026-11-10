# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/); versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

### Added
- `booto`, `write` and `windows` take the device as optional: leave it out to pick a removable disk from a numbered list. The typed confirmation still follows.
- `windows`: Windows 10/11 installer USB for UEFI PCs (FAT32 partition, files copied from the ISO, `install.wim` over 4 GiB split with `wimlib-imagex`).
- Runs on Windows: `list`, `write`, and `booto` (live system only; the report partition needs Linux). Disks are `\\.\PhysicalDriveN`.
- Releases ship a Linux AppImage and a Windows MSI next to the static Linux binary. CI, nightly and tagged releases build and test every package first, including an MSI install that makes a Booto USB on a virtual disk.

- Downloads time out when stalled, are retried and resumed, and older Booto ISOs are pruned from the cache. `GITHUB_TOKEN` authenticates release lookups.
- Release downloads carry signed build provenance (`gh attestation verify`).

### Changed
- License: GPL-3.0-or-later (was MIT).
- `booto` asks the report passphrase twice before erasing (it used to be three cryptsetup prompts after), or reads it from stdin in scripts.
- A disk with any filesystem mounted outside `/media` or `/run/media`, or active swap, is refused (was only `/`, `/boot`, `/boot/efi`).
- Windows: USB hard disks and SSDs are no longer treated as removable.
- `write` flushes every 64 MiB, so its progress shows the device and the final flush is short.

### Fixed
- `--channel nightly` no longer fails while Booto's newest release is still uploading its ISO.
- An ISO too big for the 3 GiB live partition is refused before the USB is erased.
- LUKS mappers left open on the target (by a desktop or a killed run) are closed instead of keeping the disk busy.
- Releases no longer include the WiX `.wixpdb` debug file.

## [0.1.0] - 2026-11-05

### Added
- `list`: removable disks (`--all` for every disk).
- `booto`: downloads the newest Booto release (`--channel stable|nightly`) or uses `--iso`, checks its SHA-256, and builds the BOOTO_LIVE + BOOTO_DATA layout; `--data-size`, `--no-encrypt`.
- `write`: byte-for-byte image write with SHA-256 read-back verification; URLs need `--sha256`.
- Refuses the running system's disk, non-removable disks without `--force-nonremovable`, and asks for the device path before erasing.
- CI (fmt, clippy, test), daily `nightly-build` pre-release, tagged pre-releases, `scripts/promote.sh` for the stable channel.
