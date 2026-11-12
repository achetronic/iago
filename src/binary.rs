// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! The Parakeet server binary.
//!
//! Iago does not transcribe: it downloads the server, starts it with the models
//! it manages and talks to it over HTTP. The binary lives next to the models, in
//! the data directory, so a single folder holds everything the app needs.
//!
//! Releases publish per-platform assets with a `sha256:` digest, which is what
//! the download is verified against.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::{Error, Result};
use crate::models::{digest, size_mb};
use crate::oci::{ProgressSink, Stage};

/// GitHub repository that publishes the server.
pub const REPOSITORY: &str = "achetronic/parakeet";
/// Where the release list lives.
const RELEASES_URL: &str = "https://api.github.com/repos/achetronic/parakeet/releases/latest";
/// File next to the binary holding the release tag it came from.
const VERSION_FILE: &str = "parakeet.version";

/// What is installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BinaryInfo {
    /// Where the binary lives.
    pub path: PathBuf,
    /// Whether the file exists.
    pub present: bool,
    /// Release tag recorded at install time, when we downloaded it.
    pub version: Option<String>,
    /// Short SHA-256 of the file on disk.
    pub digest: Option<String>,
    /// Size in megabytes.
    pub size_mb: u64,
}

impl BinaryInfo {
    /// Version and digest, or what is missing.
    pub fn label(&self) -> String {
        let version = self.version.as_deref().unwrap_or("unknown version");
        match &self.digest {
            Some(digest) => format!("{version} · sha256 {digest}"),
            None => "not installed".to_string(),
        }
    }
}

/// Where the binary lives: Iago's own data directory. It is never taken from
/// the machine, even when a development checkout is lying around.
pub fn path(paths: &crate::paths::Paths) -> PathBuf {
    paths.default_binary()
}

/// Reads what is on disk.
pub fn inspect(paths: &crate::paths::Paths) -> BinaryInfo {
    let path = path(paths);

    if !path.is_file() {
        return BinaryInfo {
            path,
            present: false,
            version: None,
            digest: None,
            size_mb: 0,
        };
    }

    BinaryInfo {
        version: read_version(&path),
        digest: digest(&path),
        size_mb: size_mb(&path),
        path,
        present: true,
    }
}

/// Release tag recorded next to a binary we downloaded.
fn read_version(binary: &Path) -> Option<String> {
    let path = binary.parent()?.join(VERSION_FILE);
    let raw = std::fs::read_to_string(path).ok()?;
    let tag = raw.trim().to_string();

    if tag.is_empty() { None } else { Some(tag) }
}

/// Downloads the release asset for this platform, verifies it and installs it.
///
/// Re-running replaces the binary, which is also the repair path.
pub async fn install(
    paths: &crate::paths::Paths,
    progress: Option<ProgressSink>,
) -> Result<BinaryInfo> {
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .user_agent("iago")
        .build()
        .map_err(|err| Error::Transcription(format!("cannot build the HTTP client: {err}")))?;

    let release: Release = client
        .get(RELEASES_URL)
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|err| Error::Transcription(format!("cannot reach GitHub: {err}")))?
        .json()
        .await
        .map_err(|err| Error::Transcription(format!("unexpected release shape: {err}")))?;

    let wanted = asset_name();
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == wanted)
        .ok_or_else(|| {
            Error::Transcription(format!(
                "release {} has no {wanted} asset",
                release.tag_name
            ))
        })?;

    let destination = path(paths);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| Error::Config(format!("cannot create {}: {err}", parent.display())))?;
    }

    let total = asset.size;
    let partial = destination.with_extension("download");
    let bytes = download(
        &client,
        &asset.browser_download_url,
        &partial,
        total,
        progress,
    )
    .await?;

    if let Some(expected) = &asset.digest {
        let expected = expected.trim_start_matches("sha256:");
        let actual = full_digest(&partial)?;
        if actual != expected {
            let _ = std::fs::remove_file(&partial);
            return Err(Error::Transcription(format!(
                "the downloaded parakeet does not match its digest: got {}, expected {expected}",
                &actual[..12]
            )));
        }
    }

    make_executable(&partial)?;
    std::fs::rename(&partial, &destination)
        .map_err(|err| Error::Config(format!("cannot install {}: {err}", destination.display())))?;

    if let Some(parent) = destination.parent() {
        let _ = std::fs::write(parent.join(VERSION_FILE), format!("{}\n", release.tag_name));
    }

    tracing::info!(
        version = %release.tag_name,
        bytes,
        "parakeet installed"
    );

    Ok(inspect(paths))
}

/// Platform asset name, matching what the release workflow publishes.
fn asset_name() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    let os = match std::env::consts::OS {
        "macos" => "darwin",
        other => other,
    };

    format!("parakeet-{os}-{arch}")
}

async fn download(
    client: &reqwest::Client,
    url: &str,
    partial: &Path,
    total: u64,
    progress: Option<ProgressSink>,
) -> Result<u64> {
    use futures_util::StreamExt as _;
    use std::io::Write as _;

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| Error::Transcription(format!("cannot download parakeet: {err}")))?;

    if !response.status().is_success() {
        return Err(Error::Transcription(format!(
            "cannot download parakeet: {}",
            response.status()
        )));
    }

    let mut file = std::fs::File::create(partial)
        .map_err(|err| Error::Config(format!("cannot write {}: {err}", partial.display())))?;

    let mut stream = response.bytes_stream();
    let mut written = 0u64;

    while let Some(chunk) = stream.next().await {
        let chunk =
            chunk.map_err(|err| Error::Transcription(format!("the download stopped: {err}")))?;
        file.write_all(&chunk)
            .map_err(|err| Error::Config(format!("cannot write {}: {err}", partial.display())))?;
        written += chunk.len() as u64;

        if let Some(sink) = &progress {
            sink(Stage {
                name: "parakeet".to_string(),
                bytes: written,
                total,
            });
        }
    }

    Ok(written)
}

/// Full SHA-256 in lowercase hex.
fn full_digest(path: &Path) -> Result<String> {
    use sha2::{Digest as _, Sha256};
    use std::io::Read as _;

    let mut file = std::fs::File::open(path)
        .map_err(|err| Error::Config(format!("cannot read {}: {err}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];

    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|err| Error::Config(format!("cannot read {}: {err}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    let mut permissions = std::fs::metadata(path)
        .map_err(|err| Error::Config(format!("cannot read {}: {err}", path.display())))?
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)
        .map_err(|err| Error::Config(format!("cannot chmod {}: {err}", path.display())))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

#[derive(Debug, Deserialize)]
struct Release {
    tag_name: String,
    #[serde(default)]
    assets: Vec<Asset>,
}

#[derive(Debug, Deserialize)]
struct Asset {
    name: String,
    size: u64,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_asset_name_follows_the_platform() {
        let name = asset_name();
        assert!(name.starts_with("parakeet-"), "{name}");
        assert!(name.contains(std::env::consts::OS) || name.contains("darwin"));
    }

    #[test]
    fn a_missing_binary_is_reported_as_such() {
        let paths = crate::paths::Paths::with_root("/tmp/iago-binary-test");
        let info = inspect(&paths);

        assert!(!info.present);
        assert_eq!(info.label(), "not installed");
        assert!(info.digest.is_none());
    }

    #[test]
    fn an_installed_binary_reports_its_digest_and_version() {
        let root = std::env::temp_dir().join(format!("iago-bin-{}", std::process::id()));
        let bin = root.join("data").join("bin");
        std::fs::create_dir_all(&bin).expect("dirs");

        let path = bin.join(crate::paths::binary_name());
        std::fs::write(&path, b"#!/bin/sh\necho parakeet\n").expect("write");
        std::fs::write(bin.join(VERSION_FILE), "v0.11.0\n").expect("version");

        let paths = crate::paths::Paths::with_root(&root);
        let info = inspect(&paths);

        assert!(info.present);
        assert_eq!(info.version.as_deref(), Some("v0.11.0"));
        assert_eq!(info.digest.as_deref().map(str::len), Some(12));
        assert!(info.label().starts_with("v0.11.0 · sha256 "));

        let _ = std::fs::remove_dir_all(&root);
    }
}
