//! booto-writer: download and write bootable USB media (Booto setup, or any hybrid ISO / raw image).
mod device;
mod download;
mod flash;
#[cfg(windows)]
mod win;

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
#[cfg(unix)]
use {anyhow::Context, std::process::Command};

#[derive(Parser)]
#[command(
    version,
    about = "Download and write bootable USB media: Booto setup, or any hybrid ISO / raw image"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// List disks that can be written (removable only, unless --all)
    List {
        #[arg(long)]
        all: bool,
    },
    /// Download the latest Booto ISO, verify it, and set up a Booto USB (live + report partition;
    /// live system only on Windows)
    Booto {
        /// Whole disk, e.g. /dev/sdb (Windows: \\.\PhysicalDrive2); omit to pick from a list
        device: Option<String>,
        /// stable = latest full release; nightly = newest release including pre-releases
        #[arg(long, value_enum, default_value_t = Channel::Stable)]
        channel: Channel,
        /// Use this local ISO instead of downloading one
        #[arg(long)]
        iso: Option<PathBuf>,
        /// Report partition size, e.g. 512M or 2G (minimum 64M; default: rest of the disk)
        #[arg(long)]
        data_size: Option<String>,
        /// Create the report partition without a passphrase (anyone with the USB can read reports)
        #[arg(long)]
        no_encrypt: bool,
        #[command(flatten)]
        safety: Safety,
    },
    /// Write a hybrid ISO or raw image (most Linux distributions) byte for byte, then verify it
    Write {
        /// Local path or http(s):// URL
        image: String,
        /// Whole disk, e.g. /dev/sdb (Windows: \\.\PhysicalDrive2); omit to pick from a list
        device: Option<String>,
        /// Expected SHA-256 of the image; required for URLs
        #[arg(long)]
        sha256: Option<String>,
        #[command(flatten)]
        safety: Safety,
    },
    /// Windows 10/11 installer USB for UEFI PCs (FAT32; splits install.wim over 4 GiB, needs wimtools)
    Windows {
        /// Local path or http(s):// URL of the Windows ISO
        iso: String,
        /// Whole disk, e.g. /dev/sdb; omit to pick from a list
        device: Option<String>,
        /// Expected SHA-256 of the ISO; required for URLs
        #[arg(long)]
        sha256: Option<String>,
        #[command(flatten)]
        safety: Safety,
    },
}

#[derive(clap::Args)]
struct Safety {
    /// Skip the typed confirmation
    #[arg(long)]
    yes: bool,
    /// Allow a disk that is not marked removable (some USB SSDs)
    #[arg(long)]
    force_nonremovable: bool,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum Channel {
    Stable,
    Nightly,
}

/// Runs a command and returns its stdout; fails with its stderr.
#[cfg(unix)]
pub fn run(cmd: &str, args: &[&str]) -> Result<String> {
    let out = Command::new(cmd)
        .args(args)
        .output()
        .with_context(|| format!("cannot run {cmd}"))?;
    if !out.status.success() {
        bail!(
            "{cmd} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs a command with the terminal attached (progress output, passphrase prompts).
#[cfg(unix)]
pub fn exec(cmd: &str, args: &[&str]) -> Result<()> {
    let status = Command::new(cmd)
        .args(args)
        .status()
        .with_context(|| format!("cannot run {cmd}"))?;
    if !status.success() {
        bail!("{cmd} {} failed ({status})", args.join(" "));
    }
    Ok(())
}

/// Runs a command with `input` on its stdin (a passphrase never goes on the command line).
#[cfg(unix)]
pub fn run_input(cmd: &str, args: &[&str], input: &[u8]) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;
    let mut child = Command::new(cmd)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("cannot run {cmd}"))?;
    child
        .stdin
        .take()
        .context("no stdin pipe")?
        .write_all(input)?;
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "{cmd} {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

/// Downloads a URL (checksum required) or checks a local file against an optional checksum.
fn resolve_image(image: &str, sha256: Option<String>) -> Result<PathBuf> {
    let expected = sha256
        .as_deref()
        .map(download::normalize_sha256)
        .transpose()?;
    if image.starts_with("http://") || image.starts_with("https://") {
        let Some(expected) = expected else {
            bail!("--sha256 is required when downloading from a URL");
        };
        return download::fetch(image, &expected);
    }
    let path = PathBuf::from(image);
    if let Some(expected) = expected {
        download::verify(&path, &expected)?;
    }
    Ok(path)
}

/// Root check, then the given disk or one picked from the removable list, through the safety checks.
fn target(device: Option<String>, safety: &Safety) -> Result<device::Disk> {
    device::require_root()?;
    let path = match device {
        Some(path) => path,
        None => device::pick()?,
    };
    device::prepare(&path, safety.force_nonremovable)
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::List { all } => device::print_list(all),
        Cmd::Booto {
            device,
            channel,
            iso,
            data_size,
            no_encrypt,
            safety,
        } => {
            let data_mib = data_size
                .as_deref()
                .map(flash::parse_size_mib)
                .transpose()?;
            if cfg!(windows) && (data_mib.is_some() || no_encrypt) {
                bail!(
                    "on Windows booto-writer makes the live system only; --data-size and --no-encrypt need Linux"
                );
            }
            let disk = target(device, &safety)?;
            let iso = match iso {
                Some(path) => path,
                None => download::latest_booto(channel)?,
            };
            flash::check_iso_fits(&iso)?;
            device::confirm(&disk, safety.yes)?;
            #[cfg(unix)]
            return flash::booto(&disk, &iso, data_mib, !no_encrypt);
            #[cfg(windows)]
            return win::booto_live(&disk, &iso);
        }
        Cmd::Write {
            image,
            device,
            sha256,
            safety,
        } => {
            let disk = target(device, &safety)?;
            let path = resolve_image(&image, sha256)?;
            device::confirm(&disk, safety.yes)?;
            flash::raw(&disk, &path)
        }
        Cmd::Windows {
            iso,
            device,
            sha256,
            safety,
        } => {
            #[cfg(windows)]
            {
                let _ = (iso, device, sha256, safety);
                bail!("the windows command needs Linux for now");
            }
            #[cfg(unix)]
            {
                let disk = target(device, &safety)?;
                let path = resolve_image(&iso, sha256)?;
                device::confirm(&disk, safety.yes)?;
                flash::windows(&disk, &path)
            }
        }
    }
}
