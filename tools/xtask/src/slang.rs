//! Provisioning the pinned Slang compiler (ADR-0013 §3).
//!
//! `tools/slang.json` names the version and, for each platform, the release archive and its
//! SHA-256. `cargo xtask slang install` downloads the archive (with `curl`), verifies the hash,
//! unpacks it (pure Rust, so no dependence on whichever `tar` is on `PATH`) into
//! `.tools/slang/<version>/`, and checks that `slangc -v` reports the pinned version. A marker
//! file records the verified hash, so later runs are instant.

use std::fs;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use sha2::{Digest, Sha256};

const MANIFEST: &str = "tools/slang.json";
const MARKER: &str = ".oxreal-verified";

/// One platform's pinned archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Asset {
    pub(crate) url: String,
    pub(crate) sha256: String,
}

/// The parsed `tools/slang.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub(crate) version: String,
    pub(crate) asset: Asset,
}

/// The manifest's key for the platform xtask runs on.
pub(crate) fn platform_key() -> Result<&'static str, String> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Ok("windows-x86_64")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Ok("linux-x86_64")
    } else {
        Err("no pinned Slang build for this platform; set OXREAL_SLANGC to a slangc binary".into())
    }
}

/// Parses the manifest and picks `platform`'s asset, validating every field.
pub(crate) fn parse_manifest(text: &str, platform: &str) -> Result<Manifest, String> {
    let json: Value =
        serde_json::from_str(text).map_err(|e| format!("{MANIFEST} is not valid JSON: {e}"))?;
    let version = json["version"]
        .as_str()
        .filter(|v| !v.is_empty())
        .ok_or_else(|| format!("{MANIFEST}: missing \"version\""))?
        .to_owned();
    let entry = &json["assets"][platform];
    let url = entry["url"]
        .as_str()
        .filter(|u| u.starts_with("https://"))
        .ok_or_else(|| format!("{MANIFEST}: assets.{platform}.url must be an https URL"))?
        .to_owned();
    let sha256 = entry["sha256"]
        .as_str()
        .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .ok_or_else(|| format!("{MANIFEST}: assets.{platform}.sha256 must be 64 hex digits"))?
        .to_ascii_lowercase();
    if !url.ends_with(".tar.gz") {
        return Err(format!(
            "{MANIFEST}: assets.{platform}.url must be a .tar.gz archive"
        ));
    }
    Ok(Manifest {
        version,
        asset: Asset { url, sha256 },
    })
}

/// `cargo xtask slang <install | path>`.
pub(crate) fn run(root: &Path, args: &[String]) -> Result<(), String> {
    match args.first().map(String::as_str) {
        Some("install") => {
            let slangc = ensure_installed(root)?;
            println!("slang: {}", slangc.display());
            Ok(())
        }
        Some("path") => {
            let manifest = load_manifest(root)?;
            println!("{}", slangc_path(root, &manifest.version).display());
            Ok(())
        }
        _ => Err("usage: cargo xtask slang <install | path>".to_owned()),
    }
}

fn load_manifest(root: &Path) -> Result<Manifest, String> {
    let path = root.join(MANIFEST);
    let text =
        fs::read_to_string(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    parse_manifest(&text, platform_key()?)
}

fn install_dir(root: &Path, version: &str) -> PathBuf {
    root.join(".tools").join("slang").join(version)
}

fn slangc_path(root: &Path, version: &str) -> PathBuf {
    let exe = if cfg!(windows) {
        "slangc.exe"
    } else {
        "slangc"
    };
    install_dir(root, version).join("bin").join(exe)
}

/// Installs the pinned Slang if it isn't already verified. Returns the `slangc` path.
pub(crate) fn ensure_installed(root: &Path) -> Result<PathBuf, String> {
    let manifest = load_manifest(root)?;
    let dir = install_dir(root, &manifest.version);
    let slangc = slangc_path(root, &manifest.version);
    let marker = dir.join(MARKER);
    if slangc.is_file()
        && fs::read_to_string(&marker).is_ok_and(|m| m.trim() == manifest.asset.sha256)
    {
        return Ok(slangc);
    }

    println!(
        "slang: installing {} into {}",
        manifest.version,
        dir.display()
    );
    let downloads = root.join(".tools").join("downloads");
    fs::create_dir_all(&downloads)
        .map_err(|e| format!("cannot create {}: {e}", downloads.display()))?;
    let file_name = manifest
        .asset
        .url
        .rsplit('/')
        .next()
        .ok_or("asset URL has no file name")?;
    let archive = downloads.join(file_name);

    if !archive.is_file() || sha256_file(&archive)? != manifest.asset.sha256 {
        download(&manifest.asset.url, &archive)?;
    }
    let actual = sha256_file(&archive)?;
    if actual != manifest.asset.sha256 {
        let _ = fs::remove_file(&archive);
        return Err(format!(
            "checksum mismatch for {file_name}: expected {}, got {actual}. The download was \
             deleted. If this persists, the release asset changed: do not edit the hash without \
             review (ADR-0013)",
            manifest.asset.sha256
        ));
    }

    // Unpack into a scratch directory and move it into place, so an interrupted install never
    // leaves a half-populated version directory behind.
    let staging = dir.with_extension("partial");
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging)
        .map_err(|e| format!("cannot create {}: {e}", staging.display()))?;
    unpack_tar_gz(&archive, &staging)?;
    let _ = fs::remove_dir_all(&dir);
    fs::rename(&staging, &dir).map_err(|e| {
        format!(
            "cannot move {} to {}: {e}",
            staging.display(),
            dir.display()
        )
    })?;

    check_version(&slangc, &manifest.version)?;
    fs::write(&marker, &manifest.asset.sha256)
        .map_err(|e| format!("cannot write {}: {e}", marker.display()))?;
    println!(
        "slang: {} verified (sha256 {})",
        manifest.version, manifest.asset.sha256
    );
    Ok(slangc)
}

fn download(url: &str, to: &Path) -> Result<(), String> {
    let partial = to.with_extension("partial");
    println!("slang: downloading {url}");
    let status = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--retry",
            "3",
        ])
        .args(["--proto", "=https", "--tlsv1.2", "--output"])
        .arg(&partial)
        .arg(url)
        .status()
        .map_err(|e| format!("cannot run curl (needed to download Slang): {e}"))?;
    if !status.success() {
        let _ = fs::remove_file(&partial);
        return Err(format!("downloading {url} failed ({status})"));
    }
    fs::rename(&partial, to).map_err(|e| format!("cannot move download into place: {e}"))
}

/// Lowercase hex SHA-256 of a file.
pub(crate) fn sha256_file(path: &Path) -> Result<String, String> {
    let file = fs::File::open(path).map_err(|e| format!("cannot open {}: {e}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 1 << 16];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

fn unpack_tar_gz(archive: &Path, into: &Path) -> Result<(), String> {
    let file =
        fs::File::open(archive).map_err(|e| format!("cannot open {}: {e}", archive.display()))?;
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(BufReader::new(file)));
    tar.set_preserve_permissions(true);
    // `unpack` refuses entries that would escape the target directory.
    tar.unpack(into)
        .map_err(|e| format!("cannot unpack {}: {e}", archive.display()))
}

fn check_version(slangc: &Path, version: &str) -> Result<(), String> {
    let output = Command::new(slangc)
        .arg("-v")
        .output()
        .map_err(|e| format!("cannot run {}: {e}", slangc.display()))?;
    // slangc prints its version on stderr.
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if text.contains(version) {
        Ok(())
    } else {
        Err(format!(
            "{} reports {:?}, expected version {version}",
            slangc.display(),
            text.trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"{
        "version": "2026.22",
        "assets": {
            "linux-x86_64": {
                "url": "https://example.org/slang-2026.22-linux-x86_64.tar.gz",
                "sha256": "A877B75C6EC515F5D8BF66EA4BAE4F0A5BA85050B9540C2046670285808347E6"
            }
        }
    }"#;

    #[test]
    fn parses_the_manifest_and_lowercases_the_hash() {
        let m = parse_manifest(GOOD, "linux-x86_64").unwrap();
        assert_eq!(m.version, "2026.22");
        assert_eq!(
            m.asset.sha256,
            "a877b75c6ec515f5d8bf66ea4bae4f0a5ba85050b9540c2046670285808347e6"
        );
    }

    #[test]
    fn rejects_missing_platforms_and_bad_fields() {
        assert!(parse_manifest(GOOD, "windows-x86_64").is_err());
        let http = GOOD.replace("https://", "http://");
        assert!(
            parse_manifest(&http, "linux-x86_64")
                .unwrap_err()
                .contains("https")
        );
        let short = GOOD.replace("A877B75C", "A877");
        assert!(
            parse_manifest(&short, "linux-x86_64")
                .unwrap_err()
                .contains("64 hex")
        );
        let zip = GOOD.replace(".tar.gz", ".zip");
        assert!(
            parse_manifest(&zip, "linux-x86_64")
                .unwrap_err()
                .contains(".tar.gz")
        );
        assert!(parse_manifest("{", "linux-x86_64").is_err());
    }

    #[test]
    fn the_repository_manifest_is_valid_for_every_supported_platform() {
        let text = include_str!("../../slang.json");
        for platform in ["windows-x86_64", "linux-x86_64"] {
            parse_manifest(text, platform).unwrap();
        }
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        let dir = std::env::temp_dir().join(format!("oxreal-sha-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("abc.txt");
        fs::write(&file, "abc").unwrap();
        assert_eq!(
            sha256_file(&file).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        fs::remove_dir_all(&dir).unwrap();
    }
}
