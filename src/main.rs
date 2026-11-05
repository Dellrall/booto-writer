//! booto-writer: download and write bootable USB media (Booto setup, or any hybrid ISO / raw image).
mod device;
mod download;
mod flash;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::Command;

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
    /// Download the latest Booto ISO, verify it, and set up a Booto USB (live + report partition)
    Booto {
        /// Whole disk, e.g. /dev/sdb
        device: String,
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
        /// Whole disk, e.g. /dev/sdb
        device: String,
        /// Expected SHA-256 of the image; required for URLs
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
            device::require_root()?;
            let disk = device::prepare(&device, safety.force_nonremovable)?;
            let iso = match iso {
                Some(path) => path,
                None => download::latest_booto(channel)?,
            };
            device::confirm(&disk, safety.yes)?;
            flash::booto(&disk, &iso, data_mib, !no_encrypt)
        }
        Cmd::Write {
            image,
            device,
            sha256,
            safety,
        } => {
            device::require_root()?;
            let disk = device::prepare(&device, safety.force_nonremovable)?;
            let path = if image.starts_with("http://") || image.starts_with("https://") {
                let Some(expected) = sha256 else {
                    bail!("--sha256 is required when downloading from a URL");
                };
                download::fetch(&image, &download::normalize_sha256(&expected)?)?
            } else {
                let path = PathBuf::from(&image);
                if let Some(expected) = sha256 {
                    download::verify(&path, &download::normalize_sha256(&expected)?)?;
                }
                path
            };
            device::confirm(&disk, safety.yes)?;
            flash::raw(&disk, &path)
        }
    }
}
