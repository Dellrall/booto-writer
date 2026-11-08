//! Release lookup, cached downloads and SHA-256 verification.
use crate::Channel;
use anyhow::{Context, Result, bail};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

const BOOTO_REPO: &str = "Dellrall/Booto";
const USER_AGENT: &str = concat!("booto-writer/", env!("CARGO_PKG_VERSION"));

fn get(url: &str) -> Result<ureq::http::Response<ureq::Body>> {
    ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .call()
        .with_context(|| format!("GET {url}"))
}

fn get_string(url: &str) -> Result<String> {
    Ok(get(url)?.into_body().read_to_string()?)
}

pub struct ReleaseIso {
    pub tag: String,
    pub name: String,
    pub url: String,
    pub sha256_url: String,
}

/// Picks the ISO and its .sha256 from a GitHub release object, or the first entry of a release list.
fn pick_iso(json: &str) -> Result<ReleaseIso> {
    let v: Value = serde_json::from_str(json).context("GitHub returned invalid JSON")?;
    let release = if v.is_array() { &v[0] } else { &v };
    let tag = release["tag_name"]
        .as_str()
        .context("no release found")?
        .to_string();
    let assets = release["assets"]
        .as_array()
        .context("release has no assets")?;
    let url_of = |name: &str| {
        assets
            .iter()
            .find(|a| a["name"] == name)
            .and_then(|a| a["browser_download_url"].as_str())
            .map(str::to_string)
    };
    let name = assets
        .iter()
        .filter_map(|a| a["name"].as_str())
        .find(|n| n.ends_with(".iso"))
        .with_context(|| format!("release {tag} has no .iso asset"))?
        .to_string();
    let url = url_of(&name).context("ISO asset has no download URL")?;
    let sha256_url = url_of(&format!("{name}.sha256")).with_context(|| {
        format!("release {tag} has no {name}.sha256; refusing an unverifiable ISO")
    })?;
    Ok(ReleaseIso {
        tag,
        name,
        url,
        sha256_url,
    })
}

/// Accepts a bare digest or a `sha256sum` line ("<hex>  <file>"); returns lowercase hex.
pub fn normalize_sha256(text: &str) -> Result<String> {
    let hex = text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    if hex.len() != 64 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("not a SHA-256 digest: {:?}", text.trim());
    }
    Ok(hex)
}

pub fn latest_booto(channel: Channel) -> Result<PathBuf> {
    let api = match channel {
        Channel::Stable => format!("https://api.github.com/repos/{BOOTO_REPO}/releases/latest"),
        Channel::Nightly => {
            format!("https://api.github.com/repos/{BOOTO_REPO}/releases?per_page=1")
        }
    };
    let iso = pick_iso(&get_string(&api)?)?;
    println!("Booto {}: {}", iso.tag, iso.name);
    let expected = normalize_sha256(&get_string(&iso.sha256_url)?)?;
    download_to_cache(&iso.url, &iso.name, &expected)
}

/// Downloads `url` into the cache, named after its last path segment.
pub fn fetch(url: &str, expected: &str) -> Result<PathBuf> {
    let name = url
        .split(['?', '#'])
        .next()
        .and_then(|u| u.rsplit('/').next())
        .filter(|n| !n.is_empty() && *n != "." && *n != "..")
        .unwrap_or("image.iso");
    download_to_cache(url, name, expected)
}

fn cache_dir() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .or_else(|| std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
        .context("none of XDG_CACHE_HOME, HOME or LOCALAPPDATA is set")?;
    let dir = base.join("booto-writer");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn download_to_cache(url: &str, name: &str, expected: &str) -> Result<PathBuf> {
    let path = cache_dir()?.join(name);
    if path.exists() && sha256_file(&path)? == expected {
        println!("Using cached {}", path.display());
        return Ok(path);
    }
    let part = path.with_extension("part");
    let resp = get(url)?;
    let total: Option<u64> = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok()?.parse().ok());
    let mut out = File::create(&part)?;
    let (got, _) = pump(
        resp.into_body().into_reader(),
        total,
        "Downloading",
        |chunk| Ok(out.write_all(chunk)?),
    )?;
    out.sync_all()?;
    if got != expected {
        std::fs::remove_file(&part)?;
        bail!("checksum mismatch for {name}: expected {expected}, got {got}; download deleted");
    }
    std::fs::rename(&part, &path)?;
    println!("Verified SHA-256 {got}");
    Ok(path)
}

/// Reads `r` to the end, hashing every chunk and handing it to `sink`; returns (SHA-256 hex, bytes read).
/// An empty `label` hides the progress line.
pub fn pump(
    mut r: impl Read,
    total: Option<u64>,
    label: &str,
    mut sink: impl FnMut(&[u8]) -> Result<()>,
) -> Result<(String, u64)> {
    let mut buf = vec![0u8; 4 << 20];
    let mut hasher = Sha256::new();
    let mut done = 0u64;
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        };
        sink(&buf[..n])?;
        hasher.update(&buf[..n]);
        done += n as u64;
        if !label.is_empty() {
            progress(label, done, total);
        }
    }
    if !label.is_empty() {
        eprintln!();
    }
    Ok((hex(&hasher.finalize()), done))
}

fn progress(what: &str, done: u64, total: Option<u64>) {
    let mib = done >> 20;
    match total {
        Some(t) if t > 0 => eprint!(
            "\r{what}: {mib} MiB / {} MiB ({}%)",
            t >> 20,
            done * 100 / t
        ),
        _ => eprint!("\r{what}: {mib} MiB"),
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
    Ok(pump(file, None, "", |_| Ok(()))?.0)
}

pub fn verify(path: &Path, expected: &str) -> Result<()> {
    let got = sha256_file(path)?;
    if got != expected {
        bail!(
            "checksum mismatch for {}: expected {expected}, got {got}",
            path.display()
        );
    }
    println!("Verified SHA-256 {got}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "8a1d1f0bb39b4f5b2a0f0e3c5fe6f22e33cd7fb4a7a2e4c9a0c1b4e2b9c1d2e3";

    #[test]
    fn sha256_accepts_bare_and_sha256sum_lines() {
        assert_eq!(normalize_sha256(HASH).unwrap(), HASH);
        let line = format!("{}  booto-v1.4.0-amd64.iso\n", HASH.to_uppercase());
        assert_eq!(normalize_sha256(&line).unwrap(), HASH);
        assert!(normalize_sha256("deadbeef").is_err());
        assert!(normalize_sha256(&HASH.replace('8', "g")).is_err());
        assert!(normalize_sha256("").is_err());
    }

    #[test]
    fn picks_iso_and_checksum_from_release_or_list() {
        let release = r#"{"tag_name":"v1.4.0","assets":[
            {"name":"booto-v1.4.0-amd64.iso.sha256","browser_download_url":"https://x/s"},
            {"name":"booto-v1.4.0-amd64.iso","browser_download_url":"https://x/i"}]}"#;
        let iso = pick_iso(release).unwrap();
        assert_eq!(
            (iso.tag.as_str(), iso.name.as_str()),
            ("v1.4.0", "booto-v1.4.0-amd64.iso")
        );
        assert_eq!(
            (iso.url.as_str(), iso.sha256_url.as_str()),
            ("https://x/i", "https://x/s")
        );

        let list = format!("[{release}]");
        assert_eq!(pick_iso(&list).unwrap().tag, "v1.4.0");
    }

    #[test]
    fn refuses_release_without_checksum() {
        let release =
            r#"{"tag_name":"v1","assets":[{"name":"a.iso","browser_download_url":"https://x/i"}]}"#;
        assert!(pick_iso(release).is_err());
        assert!(pick_iso("[]").is_err());
    }
}
