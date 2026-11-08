//! Disk discovery (lsblk) and the safety checks that run before anything is written.
//! On Windows the platform half comes from `crate::win`.
#[cfg(windows)]
pub use crate::win::{disks, prepare, require_root};
use anyhow::{Result, bail};
use std::io::{BufRead, Write};
#[cfg(unix)]
use {
    crate::run, anyhow::Context, serde_json::Value, std::os::unix::fs::MetadataExt,
    std::path::PathBuf,
};

pub struct Disk {
    pub path: String,
    pub size: u64,
    pub model: String,
    pub removable: bool,
    pub tran: String,
}

#[cfg(unix)]
pub fn require_root() -> Result<()> {
    // /proc/self is owned by the effective user of this process.
    if std::fs::metadata("/proc/self")?.uid() != 0 {
        bail!("writing a disk needs root; run with sudo");
    }
    Ok(())
}

#[cfg(unix)]
fn disks() -> Result<Vec<Disk>> {
    parse_lsblk(&run(
        "lsblk",
        &["-J", "-b", "-d", "-o", "PATH,SIZE,MODEL,RM,TRAN,TYPE"],
    )?)
}

#[cfg(unix)]
/// Old util-linux prints every value as a string, new versions print numbers and booleans.
fn parse_lsblk(json: &str) -> Result<Vec<Disk>> {
    let v: Value = serde_json::from_str(json).context("lsblk printed invalid JSON")?;
    let devices = v["blockdevices"]
        .as_array()
        .context("lsblk JSON has no blockdevices")?;
    Ok(devices
        .iter()
        .filter(|d| d["type"] == "disk")
        .map(|d| Disk {
            path: d["path"].as_str().unwrap_or_default().to_string(),
            size: d["size"]
                .as_u64()
                .or_else(|| d["size"].as_str()?.parse().ok())
                .unwrap_or(0),
            model: d["model"].as_str().unwrap_or_default().trim().to_string(),
            removable: d["rm"].as_bool().unwrap_or(d["rm"] == "1"),
            tran: d["tran"].as_str().unwrap_or_default().to_string(),
        })
        .collect())
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / (1u64 << 30) as f64
}

pub fn print_list(all: bool) -> Result<()> {
    let list: Vec<Disk> = disks()?
        .into_iter()
        .filter(|d| all || d.removable)
        .collect();
    if list.is_empty() {
        println!("No removable disks found (use --all to show every disk).");
        return Ok(());
    }
    println!(
        "{:<16} {:>9}  {:<5} {:<9} MODEL",
        "DEVICE", "SIZE", "TRAN", "REMOVABLE"
    );
    for d in list {
        let rm = if d.removable { "yes" } else { "no" };
        println!(
            "{:<16} {:>6.1} GiB  {:<5} {:<9} {}",
            d.path,
            gib(d.size),
            d.tran,
            rm,
            d.model
        );
    }
    Ok(())
}

#[cfg(unix)]
fn canonical(path: &str) -> Option<PathBuf> {
    std::fs::canonicalize(path).ok()
}

#[cfg(unix)]
/// Disks that back the running system: never written, even with --force-nonremovable.
fn system_disks() -> Vec<PathBuf> {
    let mut found = Vec::new();
    for mount in ["/", "/boot", "/boot/efi"] {
        let Ok(source) = run("findmnt", &["-nvro", "SOURCE", "--target", mount]) else {
            continue;
        };
        let source = source.trim();
        if !source.starts_with("/dev/") {
            continue;
        }
        // -s walks up through LVM/LUKS/RAID to the physical disks.
        let Ok(tree) = run("lsblk", &["-nsrpo", "PATH,TYPE", source]) else {
            continue;
        };
        for line in tree.lines() {
            if let Some((path, "disk")) = line.split_once(' ') {
                found.extend(canonical(path));
            }
        }
    }
    found
}

#[cfg(unix)]
/// Resolves `path` to a whole, writable disk or explains why it is refused.
pub fn prepare(path: &str, force_nonremovable: bool) -> Result<Disk> {
    let want = canonical(path).with_context(|| format!("{path} does not exist"))?;
    let disk = disks()?
        .into_iter()
        .find(|d| canonical(&d.path).as_ref() == Some(&want))
        .with_context(|| {
            format!("{path} is not a whole disk (pass e.g. /dev/sdb, not /dev/sdb1)")
        })?;
    if system_disks().contains(&want) {
        bail!("{path} holds the running system; refusing to write it");
    }
    if !disk.removable && !force_nonremovable {
        bail!("{path} is not a removable disk; pass --force-nonremovable if you are sure");
    }
    Ok(disk)
}

pub fn confirm(disk: &Disk, yes: bool) -> Result<()> {
    println!(
        "Target: {}  {:.1} GiB  {} {}",
        disk.path,
        gib(disk.size),
        disk.tran,
        disk.model
    );
    println!("ALL DATA ON {} WILL BE ERASED.", disk.path);
    if yes {
        return Ok(());
    }
    print!("Type the device path ({}) to confirm: ", disk.path);
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if answer.trim() != disk.path {
        bail!("confirmation did not match; nothing was written");
    }
    Ok(())
}

#[cfg(unix)]
/// Unmounts every mounted partition of `disk`; fails if any refuses.
pub fn unmount_all(disk: &Disk) -> Result<()> {
    for line in run("lsblk", &["-nrpo", "PATH,MOUNTPOINT", &disk.path])?.lines() {
        if let Some((part, mountpoint)) = line.split_once(' ')
            && !mountpoint.is_empty()
        {
            println!("Unmounting {part} from {mountpoint}");
            run("umount", &[part])?;
        }
    }
    Ok(())
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lsblk_old_and_new_json_formats() {
        let new = r#"{"blockdevices":[
            {"path":"/dev/sda","size":512110190592,"model":"Samsung SSD ","rm":false,"tran":"sata","type":"disk"},
            {"path":"/dev/sdb","size":16008609792,"model":"Cruzer","rm":true,"tran":"usb","type":"disk"},
            {"path":"/dev/sr0","size":1073741312,"model":"DVD","rm":true,"tran":"sata","type":"rom"}]}"#;
        let old = r#"{"blockdevices":[
            {"path":"/dev/sdb","size":"16008609792","model":"Cruzer","rm":"1","tran":"usb","type":"disk"}]}"#;

        let disks = parse_lsblk(new).unwrap();
        assert_eq!(disks.len(), 2, "rom drives are not disks");
        assert!(!disks[0].removable);
        assert_eq!(disks[0].model, "Samsung SSD");
        assert!(disks[1].removable);

        let disks = parse_lsblk(old).unwrap();
        assert_eq!(disks[0].size, 16_008_609_792);
        assert!(disks[0].removable);
    }
}
