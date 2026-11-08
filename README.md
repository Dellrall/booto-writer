# booto-writer

Command-line USB writer for [Booto](https://github.com/Dellrall/Booto) and other bootable images, written in Rust.

- **`booto`**: downloads the latest Booto ISO, checks its SHA-256, and builds the two-partition Booto USB (live system + report partition, with or without a passphrase). Same layout as Booto's `scripts/setup_usb.sh`.
- **`write`**: writes any hybrid ISO or raw image byte for byte (Debian, Ubuntu, Fedora, Arch and most other Linux ISOs), then reads the USB back to confirm it holds the image.
- **`windows`**: makes a Windows 10/11 installer USB for UEFI PCs. Windows ISOs are not hybrid images, so their files are copied to a FAT32 partition instead, and an `install.wim` over 4 GiB is split into `install.swm` parts.
- **`list`**: shows the removable disks you can write to.

Safety: it never writes the disk the running system lives on, refuses non-removable disks unless you pass `--force-nonremovable`, and asks you to type the device path before erasing anything.

## Install

x86_64 only. Every [release](https://github.com/Dellrall/booto-writer/releases) has a static Linux binary, a Linux AppImage and a Windows MSI, each with a `.sha256`:

| Channel | Release | Built from |
| :--- | :--- | :--- |
| Stable | the one marked **Latest** | `stable` branch, a few releases behind nightly |
| Nightly | `nightly-build` pre-release, rebuilt daily when code changed | `nightly` branch |

```bash
sha256sum -c booto-writer-*-x86_64-linux.sha256
chmod +x booto-writer-*-x86_64-linux && sudo mv booto-writer-*-x86_64-linux /usr/local/bin/booto-writer
```

The AppImage is the same program in one self-contained file: `chmod +x booto-writer-*.AppImage`, then `sudo ./booto-writer-*.AppImage list`.

Windows: run `booto-writer-*-x86_64-windows.msi`. It installs `booto-writer.exe` and adds it to `PATH`; use it from a terminal opened with **Run as administrator** (see [Windows](#windows)).

Or build it: `cargo build --release` (Rust 1.88 or newer).

## Use

```bash
booto-writer list

# Booto USB, newest full release, 512 MiB encrypted report partition
sudo booto-writer booto /dev/sdX --data-size 512M

# Same with Booto's newest pre-release, and a report partition without a passphrase
sudo booto-writer booto /dev/sdX --channel nightly --no-encrypt
# Booto publishes its 1.5.x releases (mouse support, quarantine export, heuristics) as -beta
# pre-releases, so --channel stable still installs v1.4.0. Use --channel nightly to get 1.5.x.

# Booto from an ISO you already have
sudo booto-writer booto /dev/sdX --iso booto-v1.4.0-amd64.iso

# Any Linux ISO: the checksum is required for URLs
sudo booto-writer write <iso-url> /dev/sdX --sha256 <sha256-from-the-distro-site>
sudo booto-writer write ubuntu-24.04-desktop-amd64.iso /dev/sdX

# Windows installer (download the ISO from microsoft.com first)
sudo booto-writer windows Win11_24H2_English_x64.iso /dev/sdX
```

Downloads are cached in `~/.cache/booto-writer` (or `$XDG_CACHE_HOME`) and reused when the checksum still matches.

The `booto` and `windows` commands need `mkfs.vfat` (dosfstools), `partprobe` (parted), `udevadm` and `blockdev`. `booto` also needs `sgdisk` (gdisk), `mkfs.ext4` (e2fsprogs) and `cryptsetup` for an encrypted report partition; `windows` needs `wimlib-imagex` (wimtools) when `install.wim` is over 4 GiB:

```bash
sudo apt-get install -y gdisk dosfstools e2fsprogs parted udev util-linux cryptsetup wimtools
```

## Windows

Disks are named `\\.\PhysicalDriveN`; `booto-writer list` shows them. `list`, `booto` and `write` work:

```powershell
booto-writer list
booto-writer booto \\.\PhysicalDrive2 --channel nightly
booto-writer write ubuntu-24.04-desktop-amd64.iso \\.\PhysicalDrive2
```

On Windows `booto` makes the live system only (the 3 GiB BOOTO_LIVE partition) and leaves the rest of the USB empty: Windows cannot create Booto's ext4 or encrypted report partition, so Booto runs without saving reports. Use booto-writer on Linux for the full two-partition USB. The `windows` command also needs Linux for now. Downloads are cached in `%LOCALAPPDATA%\booto-writer`.

## Not supported yet

- Windows installer USBs for legacy BIOS PCs (the `windows` command makes UEFI-only USBs).
- The Booto report partition and the `windows` command on Windows.
- macOS.

## Branches and releases

- `nightly` is the default branch. Changes land here through pull requests.
- A `vX.Y.Z` tag on `nightly` builds a **pre-release** (`.github/workflows/release.yml`) once every package passes its tests (`.github/workflows/package.yml`: unit tests on Linux and Windows, an AppImage smoke test, and an MSI install that makes a Booto USB on a virtual disk). Bump `version` in `Cargo.toml` first; the workflow refuses a tag that does not match.
- `scripts/promote.sh vX.Y.Z` fast-forwards `stable` to that tag and marks its release **Latest**. Promote a release once it has proven itself, so `stable` stays a few releases behind.
- `.github/workflows/nightly.yml` rebuilds the `nightly-build` pre-release every day at 03:00 UTC, only when `nightly` has new commits.

## Development

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

CI runs those three checks, and builds and tests every package, on pull requests and on pushes to `nightly` and `stable`. To save CI minutes it skips changes that touch only Markdown, `docs/` or `LICENSE`, and any commit whose message contains `[skip ci]`.

## License

MIT, see [LICENSE](LICENSE).
