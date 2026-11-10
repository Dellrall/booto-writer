//! Windows backend: disk listing, safety checks and the live-only Booto USB, through PowerShell's
//! Storage cmdlets (Get-Disk, New-Partition, Format-Volume, Mount-DiskImage).
use crate::device::Disk;
use crate::flash::LIVE_MIB;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

const ESP_GUID: &str = "{c12a7328-f81f-11d2-ba4b-00a0c93ec93b}";
const PATH_PREFIX: &str = r"\\.\PhysicalDrive";

/// Runs a PowerShell script and returns its output. `args` reach the script as $env:BW_ARG0,
/// $env:BW_ARG1..., so paths never need quoting.
fn ps(script: &str, args: &[&str]) -> Result<String> {
    let mut cmd = Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-Command",
        &format!(
            "$ErrorActionPreference = 'Stop'; $ProgressPreference = 'SilentlyContinue'; {script}"
        ),
    ]);
    for (i, arg) in args.iter().enumerate() {
        cmd.env(format!("BW_ARG{i}"), arg);
    }
    let out = cmd.output().context("cannot run powershell.exe")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let out = String::from_utf8_lossy(&out.stdout);
        bail!("PowerShell failed: {}", format!("{err}{out}").trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

pub fn require_root() -> Result<()> {
    let admin = ps(
        "([Security.Principal.WindowsPrincipal][Security.Principal.WindowsIdentity]::GetCurrent())\
         .IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)",
        &[],
    )?;
    if admin != "True" {
        bail!("writing a disk needs Administrator rights; run from an Administrator terminal");
    }
    Ok(())
}

/// BusType is an enum, so it is turned into its name; @() keeps a single disk a JSON array.
/// Win32_DiskDrive.MediaType tells a USB stick ("Removable Media") from a USB hard disk or SSD
/// ("External hard disk media"); Get-Disk cannot.
const LIST: &str = "$media = @{}; Get-CimInstance Win32_DiskDrive | ForEach-Object { \
    $media[[int]$_.Index] = \"$($_.MediaType)\" }; \
    ConvertTo-Json -Compress -InputObject @(Get-Disk | ForEach-Object { \
    [pscustomobject]@{ Number = $_.Number; Size = $_.Size; Model = \"$($_.FriendlyName)\"; \
    Bus = \"$($_.BusType)\"; Media = $media[[int]$_.Number]; System = [bool]($_.IsBoot -or $_.IsSystem) } })";

/// Parses LIST output into disks, each with whether it backs the running system.
/// Removable, as on Linux: SD and MMC cards, and USB disks that call themselves removable media.
fn parse_disks(json: &str) -> Result<Vec<(Disk, bool)>> {
    let v: Value = serde_json::from_str(json).context("Get-Disk printed invalid JSON")?;
    let list = v.as_array().context("Get-Disk JSON is not a list")?;
    Ok(list
        .iter()
        .filter_map(|d| {
            let bus = d["Bus"].as_str().unwrap_or_default();
            let removable_media = d["Media"].as_str() == Some("Removable Media");
            let disk = Disk {
                path: format!("{PATH_PREFIX}{}", d["Number"].as_u64()?),
                size: d["Size"].as_u64().unwrap_or(0),
                model: d["Model"].as_str().unwrap_or_default().trim().to_string(),
                removable: matches!(bus, "SD" | "MMC") || (bus == "USB" && removable_media),
                tran: bus.to_ascii_lowercase(),
            };
            Some((disk, d["System"].as_bool().unwrap_or(true)))
        })
        .collect())
}

pub fn disks() -> Result<Vec<Disk>> {
    Ok(parse_disks(&ps(LIST, &[])?)?
        .into_iter()
        .map(|(d, _)| d)
        .collect())
}

/// Resolves `path` (\\.\PhysicalDriveN) to a writable disk or explains why it is refused.
pub fn prepare(path: &str, force_nonremovable: bool) -> Result<Disk> {
    let (disk, system) = parse_disks(&ps(LIST, &[])?)?
        .into_iter()
        .find(|(d, _)| d.path.eq_ignore_ascii_case(path))
        .with_context(|| {
            format!(
                "{path} is not a disk; pass a path from `booto-writer list`, e.g. {PATH_PREFIX}2"
            )
        })?;
    if system {
        bail!("{path} holds the running system; refusing to write it");
    }
    if !disk.removable && !force_nonremovable {
        bail!("{path} is not a removable disk; pass --force-nonremovable if you are sure");
    }
    Ok(disk)
}

/// A drive letter as PowerShell prints it; a volume without one prints a NUL character.
fn is_letter(s: &str) -> bool {
    s.len() == 1 && s.chars().all(|c| c.is_ascii_alphabetic())
}

fn number(disk: &Disk) -> &str {
    &disk.path[PATH_PREFIX.len()..]
}

/// Brings the disk online and writable and deletes every partition, which dismounts its volumes.
pub fn clear(disk: &Disk) -> Result<()> {
    ps(
        "$d = Get-Disk -Number ([int]$env:BW_ARG0); \
         if ($d.IsOffline) { Set-Disk -Number $d.Number -IsOffline $false }; \
         if ($d.IsReadOnly) { Set-Disk -Number $d.Number -IsReadOnly $false }; \
         if ($d.PartitionStyle -ne 'RAW') { Clear-Disk -Number $d.Number -RemoveData -RemoveOEM -Confirm:$false }",
        &[number(disk)],
    )?;
    Ok(())
}

/// Dismounts the ISO when dropped.
struct IsoMount<'a>(&'a str);

impl Drop for IsoMount<'_> {
    fn drop(&mut self) {
        let _ = ps(
            "Dismount-DiskImage -ImagePath $env:BW_ARG0 | Out-Null",
            &[self.0],
        );
    }
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    for entry in
        std::fs::read_dir(from).with_context(|| format!("cannot read {}", from.display()))?
    {
        let entry = entry?;
        let dst = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            std::fs::create_dir_all(&dst)?;
            copy_tree(&entry.path(), &dst)?;
        } else {
            std::fs::copy(entry.path(), &dst)
                .with_context(|| format!("cannot copy {}", entry.path().display()))?;
        }
    }
    Ok(())
}

/// Booto's BOOTO_LIVE partition (3 GiB FAT32, EFI system partition type, the ISO's files) without
/// the BOOTO_DATA report partition: Windows cannot make ext4 or LUKS2. The rest of the disk stays free.
pub fn booto_live(disk: &Disk, iso: &Path) -> Result<()> {
    let need_mib = LIVE_MIB + 2;
    if disk.size >> 20 < need_mib {
        bail!(
            "{} has {} MiB; the live system needs at least {need_mib} MiB",
            disk.path,
            disk.size >> 20
        );
    }
    let iso = std::path::absolute(iso)?;
    let iso = iso.to_str().context("ISO path is not UTF-8")?;
    let n = number(disk);

    println!("[1/3] Partitioning {}", disk.path);
    clear(disk)?;
    let made = ps(
        &format!(
            "$n = [int]$env:BW_ARG0; Initialize-Disk -Number $n -PartitionStyle GPT; \
             $p = New-Partition -DiskNumber $n -Size {LIVE_MIB}MB -AssignDriveLetter; \
             Format-Volume -Partition $p -FileSystem FAT32 -NewFileSystemLabel BOOTO_LIVE -Confirm:$false | Out-Null; \
             \"$($p.PartitionNumber) $((Get-Partition -DiskNumber $n -PartitionNumber $p.PartitionNumber).DriveLetter)\""
        ),
        &[n],
    )?;
    let (part, letter) = made
        .rsplit_once(' ')
        .filter(|(_, l)| is_letter(l))
        .with_context(|| format!("new partition got no drive letter ({made:?})"))?;
    let part = part.trim().to_string();
    let usb = format!("{letter}:\\");

    println!("[2/3] Copying the live system to {usb}");
    let src = ps(
        "(Mount-DiskImage -ImagePath $env:BW_ARG0 -PassThru | Get-Volume).DriveLetter",
        &[iso],
    )?;
    {
        let _mounted = IsoMount(iso);
        if !is_letter(&src) {
            bail!("the mounted ISO got no drive letter ({src:?})");
        }
        copy_tree(Path::new(&format!("{src}:\\")), Path::new(&usb))?;
    }

    println!("[3/3] Marking partition {part} as an EFI system partition");
    ps(
        &format!(
            "$n = [int]$env:BW_ARG0; $p = [int]$env:BW_ARG1; Write-VolumeCache -DriveLetter $env:BW_ARG2; \
             Remove-PartitionAccessPath -DiskNumber $n -PartitionNumber $p -AccessPath \"$($env:BW_ARG2):\\\"; \
             Set-Partition -DiskNumber $n -PartitionNumber $p -GptType '{ESP_GUID}'"
        ),
        &[n, &part, letter],
    )?;
    println!(
        "Done: {} is a Booto USB (live system only). Reports are not saved: make the report \
         partition with booto-writer on Linux to keep them.",
        disk.path
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_disk_json() {
        let json = r#"[{"Number":0,"Size":512110190592,"Model":"Samsung SSD ","Bus":"NVMe","Media":"Fixed hard disk media","System":true},
            {"Number":2,"Size":16008609792,"Model":"Cruzer","Bus":"USB","Media":"Removable Media","System":false},
            {"Number":3,"Size":2000398934016,"Model":"WD Elements","Bus":"USB","Media":"External hard disk media","System":false},
            {"Size":1}]"#;
        let disks = parse_disks(json).unwrap();
        assert_eq!(disks.len(), 3, "an entry without a number is skipped");
        assert!(!disks[2].0.removable, "a USB hard disk is not removable");
        let (sys, usb) = (&disks[0], &disks[1]);
        assert!(sys.1 && !sys.0.removable);
        assert_eq!(sys.0.model, "Samsung SSD");
        assert_eq!(usb.0.path, r"\\.\PhysicalDrive2");
        assert!(!usb.1 && usb.0.removable);
        assert_eq!(number(&usb.0), "2");
        assert!(parse_disks("[]").unwrap().is_empty());
    }
}
