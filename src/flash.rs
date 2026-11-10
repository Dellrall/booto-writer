//! Writing media: raw byte-for-byte images, and the two-partition Booto layout.
use crate::device::Disk;
use crate::download::pump;
use anyhow::{Context, Result, bail};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::Path;
#[cfg(unix)]
use {
    crate::device::unmount_all,
    crate::{exec, run},
    std::os::unix::fs::{DirBuilderExt, PermissionsExt},
    std::path::PathBuf,
};

/// Size of the BOOTO_LIVE partition, as in Booto's scripts/setup_usb.sh.
pub const LIVE_MIB: u64 = 3 * 1024;
/// Raw disks take whole sectors only (Windows refuses anything else).
const SECTOR: usize = 512;
const MIN_DATA_MIB: u64 = 64;
/// Room FAT32 needs on BOOTO_LIVE besides the ISO's files (two FATs, directories, cluster slack).
const FAT_OVERHEAD_MIB: u64 = 32;

/// Fails early when the ISO's files cannot fit the 3 GiB BOOTO_LIVE partition.
pub fn check_iso_fits(iso: &Path) -> Result<()> {
    let len = std::fs::metadata(iso)
        .with_context(|| format!("cannot open {}", iso.display()))?
        .len();
    let room = (LIVE_MIB - FAT_OVERHEAD_MIB) << 20;
    if len > room {
        bail!(
            "{} is {} MiB; the BOOTO_LIVE partition holds at most {} MiB of files",
            iso.display(),
            len >> 20,
            room >> 20
        );
    }
    Ok(())
}

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

#[cfg(unix)]
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
    #[cfg(unix)]
    unmount_all(disk)?;
    // Windows only lets a disk be written once no volume on it is mounted.
    #[cfg(windows)]
    crate::win::clear(disk)?;

    let src = File::open(image).with_context(|| format!("cannot open {}", image.display()))?;
    let dst = OpenOptions::new()
        .write(true)
        .open(&disk.path)
        .with_context(|| format!("cannot open {}", disk.path))?;
    let mut dst = SectorWriter::new(dst);
    let (written, _) = pump(src, Some(len), "Writing", |chunk| dst.write(chunk))?;
    println!("Flushing to the device...");
    let dst = dst.finish()?;
    dst.sync_all()?;
    drop(dst);

    // Drop cached pages so the read-back comes from the device, not memory.
    #[cfg(unix)]
    run("blockdev", &["--flushbufs", &disk.path])?;
    let dev = SectorReader::new(File::open(&disk.path)?, len);
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

/// Passes on whole sectors only; `finish` pads the last one with zeros.
struct SectorWriter<W: Write> {
    inner: W,
    pending: Vec<u8>,
}

impl<W: Write> SectorWriter<W> {
    fn new(inner: W) -> Self {
        Self {
            inner,
            pending: Vec::new(),
        }
    }

    fn write(&mut self, data: &[u8]) -> Result<()> {
        self.pending.extend_from_slice(data);
        let whole = self.pending.len() / SECTOR * SECTOR;
        self.inner.write_all(&self.pending[..whole])?;
        self.pending.drain(..whole);
        Ok(())
    }

    fn finish(mut self) -> Result<W> {
        if !self.pending.is_empty() {
            self.pending.resize(SECTOR, 0);
            self.inner.write_all(&self.pending)?;
        }
        self.inner.flush()?;
        Ok(self.inner)
    }
}

/// Reads the first `len` bytes of a disk while asking it for whole sectors only.
struct SectorReader<R: Read> {
    inner: R,
    left: u64,
    buf: Vec<u8>,
    pos: usize,
    end: usize,
}

impl<R: Read> SectorReader<R> {
    fn new(inner: R, len: u64) -> Self {
        Self {
            inner,
            left: len,
            buf: vec![0; 4 << 20],
            pos: 0,
            end: 0,
        }
    }
}

impl<R: Read> Read for SectorReader<R> {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos == self.end {
            if self.left == 0 {
                return Ok(0);
            }
            let want = (self.left.min(self.buf.len() as u64) as usize).next_multiple_of(SECTOR);
            let n = self.inner.read(&mut self.buf[..want])?;
            let n = n.min(self.left as usize);
            self.left -= n as u64;
            (self.pos, self.end) = (0, n);
            if n == 0 {
                self.left = 0;
                return Ok(0);
            }
        }
        let n = out.len().min(self.end - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

#[cfg(unix)]
/// Unmounts (and removes) a temporary mount point when dropped, so an error never leaves it mounted.
struct TempMount(PathBuf);

#[cfg(unix)]
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

#[cfg(unix)]
impl Drop for TempMount {
    fn drop(&mut self) {
        if let Some(dir) = self.0.to_str() {
            let _ = run("umount", &[dir]);
        }
        let _ = std::fs::remove_dir(&self.0);
    }
}

#[cfg(unix)]
/// Closes the LUKS mapper when dropped.
struct Mapper(&'static str);

#[cfg(unix)]
impl Drop for Mapper {
    fn drop(&mut self) {
        let _ = run("cryptsetup", &["close", self.0]);
    }
}

#[cfg(unix)]
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
    require_tools(&tools)?;
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

#[cfg(unix)]
fn require_tools(tools: &[&str]) -> Result<()> {
    for tool in tools {
        run("sh", &["-c", &format!("command -v {tool}")])
            .with_context(|| format!("required tool {tool} is not installed"))?;
    }
    Ok(())
}

#[cfg(unix)]
/// FAT32 cannot hold a file of 4 GiB or more.
const FAT32_MAX_FILE: u64 = (4 << 30) - 1;

#[cfg(unix)]
/// Windows installer USB for UEFI PCs: one FAT32 partition holding the ISO's files.
/// Windows ISOs are not hybrid images, so they cannot be written raw. An install.wim too big
/// for FAT32 is split into install.swm parts, which Windows Setup reads natively.
pub fn windows(disk: &Disk, iso: &Path) -> Result<()> {
    require_tools(&[
        "wipefs",
        "parted",
        "partprobe",
        "udevadm",
        "mkfs.vfat",
        "mount",
        "umount",
        "cp",
        "blockdev",
    ])?;
    let iso_len = std::fs::metadata(iso)?.len();
    if iso_len + (64 << 20) > disk.size {
        bail!(
            "{} has {} MiB; this ISO needs about {} MiB",
            disk.path,
            disk.size >> 20,
            (iso_len >> 20) + 64
        );
    }
    let iso = iso.to_str().context("ISO path is not UTF-8")?;
    unmount_all(disk)?;

    println!("[1/3] Partitioning {}", disk.path);
    run("wipefs", &["-af", &disk.path])?;
    run(
        "parted",
        &[
            "-s", &disk.path, "mklabel", "msdos", "mkpart", "primary", "fat32", "1MiB", "100%",
            "set", "1", "boot", "on",
        ],
    )?;
    run("partprobe", &[&disk.path])?;
    run("udevadm", &["settle"])?;
    let part = part_path(&disk.path, 1);
    if !Path::new(&part).exists() {
        bail!("partition {part} did not appear after partitioning");
    }
    run("mkfs.vfat", &["-F", "32", "-n", "WINSETUP", &part])?;

    println!("[2/3] Copying installer files");
    let iso_mnt = TempMount::new(iso, &["-o", "loop,ro"])?;
    let usb = TempMount::new(&part, &[])?;
    let wim = iso_mnt.0.join("sources/install.wim");
    let split = wim.metadata().is_ok_and(|m| m.len() > FAT32_MAX_FILE);
    if split {
        require_tools(&["wimlib-imagex"])
            .context("install.wim is over 4 GiB; install wimtools to split it")?;
    }
    for entry in std::fs::read_dir(&iso_mnt.0)? {
        let entry = entry?.path();
        if split && entry == wim.parent().unwrap_or(&wim) {
            let sources = usb.0.join("sources");
            std::fs::create_dir_all(&sources)?;
            for child in std::fs::read_dir(&entry)? {
                let child = child?.path();
                if child != wim {
                    exec(
                        "cp",
                        &["-r", &child.to_string_lossy(), &sources.to_string_lossy()],
                    )?;
                }
            }
        } else {
            exec(
                "cp",
                &["-r", &entry.to_string_lossy(), &usb.0.to_string_lossy()],
            )?;
        }
    }
    if split {
        println!("[3/3] Splitting install.wim into 3800 MiB parts");
        let swm = usb.0.join("sources/install.swm");
        exec(
            "wimlib-imagex",
            &[
                "split",
                &wim.to_string_lossy(),
                &swm.to_string_lossy(),
                "3800",
            ],
        )?;
    } else {
        println!("[3/3] install.wim fits FAT32; no split needed");
    }
    drop(usb);
    drop(iso_mnt);
    run("blockdev", &["--flushbufs", &disk.path])?;
    println!(
        "Done: {} is a Windows installer USB. Boot it in UEFI mode.",
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
    fn sector_writer_pads_and_reader_trims() {
        let image: Vec<u8> = (0..1300u32).map(|i| i as u8).collect();
        let mut w = SectorWriter::new(Vec::new());
        for chunk in image.chunks(700) {
            w.write(chunk).unwrap();
        }
        let disk = w.finish().unwrap();
        assert_eq!(disk.len(), 1536, "padded to whole sectors");
        assert_eq!(&disk[..1300], &image[..]);
        assert!(disk[1300..].iter().all(|&b| b == 0));

        let mut back = Vec::new();
        SectorReader::new(&disk[..], 1300)
            .read_to_end(&mut back)
            .unwrap();
        assert_eq!(back, image);
        let mut short = Vec::new();
        SectorReader::new(&disk[..512], 1300)
            .read_to_end(&mut short)
            .unwrap();
        assert_eq!(short.len(), 512, "a short device stops early");
    }

    #[cfg(unix)]
    #[test]
    fn partition_paths() {
        assert_eq!(part_path("/dev/sdb", 2), "/dev/sdb2");
        assert_eq!(part_path("/dev/nvme0n1", 1), "/dev/nvme0n1p1");
        assert_eq!(part_path("/dev/mmcblk0", 2), "/dev/mmcblk0p2");
    }
}
