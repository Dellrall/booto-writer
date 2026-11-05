//! Writing media: raw byte-for-byte images, and the two-partition Booto layout.
use crate::device::{Disk, unmount_all};
use crate::download::pump;
use crate::{exec, run};
use anyhow::{Context, Result, bail};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};

const LIVE_MIB: u64 = 3 * 1024;
const MIN_DATA_MIB: u64 = 64;

/// "512M" / "2G" -> MiB. The report partition needs at least 64 MiB (16 MiB of it is the LUKS2 header).
pub fn parse_size_mib(size: &str) -> Result<u64> {
    let (num, unit) = size.split_at(size.len().saturating_sub(1));
    let n: u64 = num
        .parse()
        .ok()
        .filter(|_| !num.is_empty())
        .with_context(|| format!("size must look like 512M or 2G, got {size:?}"))?;
    let mib = match unit {
        "M" => n,
        "G" => n.checked_mul(1024).context("size too large")?,
        _ => bail!("size must look like 512M or 2G, got {size:?}"),
    };
    if mib < MIN_DATA_MIB {
        bail!("report partition must be at least {MIN_DATA_MIB}M");
    }
    Ok(mib)
}

/// /dev/sdb -> /dev/sdb1; /dev/nvme0n1, /dev/mmcblk0 -> ...p1.
pub fn part_path(disk: &str, n: u32) -> String {
    if disk.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{disk}p{n}")
    } else {
        format!("{disk}{n}")
    }
}

/// Writes `image` to the disk, then reads the disk back and compares SHA-256.
pub fn raw(disk: &Disk, image: &Path) -> Result<()> {
    let len = std::fs::metadata(image)?.len();
    if len > disk.size {
        bail!(
            "image is {} MiB but {} has only {} MiB",
            len >> 20,
            disk.path,
            disk.size >> 20
        );
    }
    unmount_all(disk)?;

    let src = File::open(image).with_context(|| format!("cannot open {}", image.display()))?;
    let mut dst = OpenOptions::new()
        .write(true)
        .open(&disk.path)
        .with_context(|| format!("cannot open {}", disk.path))?;
    let (written, _) = pump(src, Some(len), "Writing", |chunk| Ok(dst.write_all(chunk)?))?;
    println!("Flushing to the device...");
    dst.sync_all()?;
    drop(dst);

    // Drop cached pages so the read-back comes from the device, not memory.
    run("blockdev", &["--flushbufs", &disk.path])?;
    let dev = File::open(&disk.path)?.take(len);
    let (read_back, done) = pump(dev, Some(len), "Verifying", |_| Ok(()))?;
    if done != len || read_back != written {
        bail!(
            "verification failed: the device does not hold the image (bad or fake-capacity USB?)"
        );
    }
    println!(
        "Done: {} written and verified (SHA-256 {written}).",
        disk.path
    );
    Ok(())
}

/// Unmounts (and removes) a temporary mount point when dropped, so an error never leaves it mounted.
struct TempMount(PathBuf);

impl TempMount {
    fn new(source: &str, options: &[&str]) -> Result<Self> {
        let dir = std::env::temp_dir().join(format!(
            "booto-writer-{}-{}",
            std::process::id(),
            source.replace('/', "_")
        ));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        let target = dir.to_str().context("temporary path is not UTF-8")?;
        let mut args = options.to_vec();
        args.extend([source, target]);
        if let Err(e) = run("mount", &args) {
            let _ = std::fs::remove_dir(&dir);
            return Err(e);
        }
        Ok(Self(dir))
    }
}

impl Drop for TempMount {
    fn drop(&mut self) {
        if let Some(dir) = self.0.to_str() {
            let _ = run("umount", &[dir]);
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

/// Closes the LUKS mapper when dropped.
struct Mapper(&'static str);

impl Drop for Mapper {
    fn drop(&mut self) {
        let _ = run("cryptsetup", &["close", self.0]);
    }
}

/// The layout Booto's scripts/setup_usb.sh makes: BOOTO_LIVE (FAT32, ISO files) + BOOTO_DATA (reports).
pub fn booto(disk: &Disk, iso: &Path, data_mib: Option<u64>, encrypt: bool) -> Result<()> {
    let mut tools = vec![
        "wipefs",
        "sgdisk",
        "partprobe",
        "udevadm",
        "mkfs.vfat",
        "mkfs.ext4",
        "mount",
        "umount",
        "cp",
        "blockdev",
    ];
    if encrypt {
        tools.push("cryptsetup");
    }
    for tool in tools {
        run("sh", &["-c", &format!("command -v {tool}")])
            .with_context(|| format!("required tool {tool} is not installed"))?;
    }
    let need_mib = LIVE_MIB + data_mib.unwrap_or(MIN_DATA_MIB) + 2;
    if disk.size >> 20 < need_mib {
        bail!(
            "{} has {} MiB; this layout needs at least {need_mib} MiB",
            disk.path,
            disk.size >> 20
        );
    }
    let iso = iso.to_str().context("ISO path is not UTF-8")?;
    unmount_all(disk)?;

    println!("[1/4] Partitioning {}", disk.path);
    run("wipefs", &["-af", &disk.path])?;
    run("sgdisk", &["--zap-all", &disk.path])?;
    run(
        "sgdisk",
        &[
            "-n",
            &format!("1:2048:+{LIVE_MIB}M"),
            "-t",
            "1:EF00",
            "-c",
            "1:BOOTO_LIVE",
            &disk.path,
        ],
    )?;
    let end = data_mib.map_or("0".to_string(), |m| format!("+{m}M"));
    let kind = if encrypt { "2:8309" } else { "2:8300" };
    run(
        "sgdisk",
        &[
            "-n",
            &format!("2:0:{end}"),
            "-t",
            kind,
            "-c",
            "2:BOOTO_DATA",
            &disk.path,
        ],
    )?;
    run("partprobe", &[&disk.path])?;
    run("udevadm", &["settle"])?;
    let (live, data) = (part_path(&disk.path, 1), part_path(&disk.path, 2));
    for part in [&live, &data] {
        if !Path::new(part).exists() {
            bail!("partition {part} did not appear after partitioning");
        }
    }

    println!("[2/4] Copying the live system to {live}");
    run("mkfs.vfat", &["-F", "32", "-n", "BOOTO_LIVE", &live])?;
    {
        let iso_mnt = TempMount::new(iso, &["-o", "loop,ro"])?;
        let live_mnt = TempMount::new(&live, &[])?;
        let from = format!("{}/.", iso_mnt.0.display());
        exec("cp", &["-r", &from, &live_mnt.0.to_string_lossy()])?;
    }

    println!("[3/4] Creating the report partition on {data}");
    let mut _mapper = None;
    let mut fs_dev = data.clone();
    if encrypt {
        println!("Choose the passphrase that protects your reports (asked twice):");
        exec(
            "cryptsetup",
            &[
                "luksFormat",
                "--type",
                "luks2",
                "--batch-mode",
                "--verify-passphrase",
                &data,
            ],
        )?;
        println!("Enter it once more to open the partition:");
        exec("cryptsetup", &["open", &data, "booto_writer_init"])?;
        _mapper = Some(Mapper("booto_writer_init"));
        fs_dev = "/dev/mapper/booto_writer_init".to_string();
    }
    // 1 KiB blocks + inline_data: a report and its seal take ~2 KiB instead of 16; -m 0 drops the root reserve.
    run(
        "mkfs.ext4",
        &[
            "-q",
            "-L",
            "persistence",
            "-T",
            "small",
            "-O",
            "inline_data",
            "-m",
            "0",
            &fs_dev,
        ],
    )?;

    println!("[4/4] Creating report folders");
    {
        let mnt = TempMount::new(&fs_dev, &[])?;
        std::fs::write(mnt.0.join("persistence.conf"), "/ union\n")?;
        for dir in ["cases", "rules", "logs"] {
            let path = mnt.0.join(dir);
            std::fs::create_dir(&path)?;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    drop(_mapper);
    run("blockdev", &["--flushbufs", &disk.path])?;

    let lock = if encrypt {
        "encrypted; Booto asks for the passphrase at start"
    } else {
        "no passphrase; Booto mounts it at start"
    };
    println!(
        "Done: {} is a Booto USB. Reports partition {data}: {lock}.",
        disk.path
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_parsing_and_minimum() {
        assert_eq!(parse_size_mib("512M").unwrap(), 512);
        assert_eq!(parse_size_mib("2G").unwrap(), 2048);
        assert_eq!(parse_size_mib("64M").unwrap(), 64);
        for bad in [
            "63M",
            "512",
            "M",
            "",
            "2T",
            "-1G",
            "1.5G",
            "99999999999999999999G",
        ] {
            assert!(parse_size_mib(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn partition_paths() {
        assert_eq!(part_path("/dev/sdb", 2), "/dev/sdb2");
        assert_eq!(part_path("/dev/nvme0n1", 1), "/dev/nvme0n1p1");
        assert_eq!(part_path("/dev/mmcblk0", 2), "/dev/mmcblk0p2");
    }
}
