# Changelog

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.0.0/); versions follow [Semantic Versioning](https://semver.org/).

## [Unreleased]

## [0.1.0] - 2026-11-05

### Added
- `list`: removable disks (`--all` for every disk).
- `booto`: downloads the newest Booto release (`--channel stable|nightly`) or uses `--iso`, checks its SHA-256, and builds the BOOTO_LIVE + BOOTO_DATA layout; `--data-size`, `--no-encrypt`.
- `write`: byte-for-byte image write with SHA-256 read-back verification; URLs need `--sha256`.
- Refuses the running system's disk, non-removable disks without `--force-nonremovable`, and asks for the device path before erasing.
- CI (fmt, clippy, test), daily `nightly-build` pre-release, tagged pre-releases, `scripts/promote.sh` for the stable channel.
