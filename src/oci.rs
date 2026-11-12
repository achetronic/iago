// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! Minimal OCI registry client.
//!
//! Enough to pull one anonymous public image from a registry and unpack its
//! `models/` subtree, without Docker, Podman or `crane` on the user's machine.
//!
//! What it does:
//!
//! 1. Asks for an anonymous pull token.
//! 2. Reads the manifest, following an image index to the `linux/amd64` entry.
//! 3. Streams each layer, gunzips it and extracts the files under `models/`,
//!    stripping that prefix (the same thing `tar --strip-components=1 models`
//!    does in the Parakeet Makefile).
//!
//! Layers are streamed, never buffered: the ASR layer is ~875 MB.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use flate2::read::GzDecoder;
use futures_util::StreamExt;
use serde::Deserialize;
use tokio_util::io::{StreamReader, SyncIoBridge};

use crate::error::{Error, Result};

/// Registry the model images live in.
pub const REGISTRY: &str = "ghcr.io";
/// Tag pulled when downloading models.
pub const TAG: &str = "latest";
/// Prefix every file in a model image carries.
const MODELS_PREFIX: &str = "models/";

/// How the caller follows the download.
pub type ProgressSink = Arc<dyn Fn(Stage) + Send + Sync>;

/// One step of a multi-image download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stage {
    /// What is being downloaded, for example `asr/int8`.
    pub name: String,
    /// Bytes of this image already written.
    pub bytes: u64,
    /// Total bytes of this image.
    pub total: u64,
}

/// What ended up on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PulledImage {
    /// Repository that was pulled.
    pub repository: String,
    /// Digest of the image manifest the layers came from.
    pub digest: String,
    /// Files written, relative to the destination.
    pub files: Vec<String>,
    /// Bytes written.
    pub bytes: u64,
}

/// Pulls `repository:tag` and extracts its `models/` subtree into `dest`.
///
/// `skip_prefixes` drops files whose name (after the `models/` prefix) starts
/// with any of them. Iago uses it to leave the full-attention encoder behind:
/// the server only loads the local-attention one and it is 650 MB.
pub async fn pull_models(
    client: &reqwest::Client,
    repository: &str,
    tag: &str,
    dest: &Path,
    label: &str,
    skip_prefixes: &[&str],
    progress: Option<ProgressSink>,
) -> Result<PulledImage> {
    let token = anonymous_token(client, repository).await?;
    let manifest = fetch_manifest(client, repository, tag, &token).await?;
    let digest = manifest.digest.clone();
    let total: u64 = manifest.layers.iter().map(|layer| layer.size).sum();

    // Extract beside the destination and swap it in at the end: a cancelled or
    // failed download must never leave a truncated model that looks installed.
    let staging = staging_dir(dest);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|err| Error::Config(format!("cannot create {}: {err}", staging.display())))?;

    let mut files = Vec::new();
    let mut written = 0u64;

    for layer in manifest.layers {
        let response = client
            .get(blob_url(repository, &layer.digest))
            .bearer_auth(&token)
            .send()
            .await
            .map_err(Error::Http)?;

        if !response.status().is_success() {
            return Err(Error::Transcription(format!(
                "cannot download layer {} of {repository}: {}",
                layer.digest,
                response.status()
            )));
        }

        let counter = Arc::new(AtomicU64::new(0));
        let counter_for_stream = Arc::clone(&counter);
        let stream = response.bytes_stream().map(move |chunk| {
            chunk
                .inspect(|chunk| {
                    counter_for_stream.fetch_add(chunk.len() as u64, Ordering::Relaxed);
                })
                .map_err(std::io::Error::other)
        });

        let reader = StreamReader::new(stream);
        let dest = staging.clone();
        let skip: Vec<String> = skip_prefixes.iter().map(|p| (*p).to_string()).collect();
        let counter_for_task = Arc::clone(&counter);
        let progress_for_task = progress.clone();
        let label = label.to_string();
        let base = written;

        // tar + gzip are blocking codecs, so the extraction runs on a blocking
        // thread with the async stream bridged into a reader.
        let extracted = tokio::task::spawn_blocking(move || -> Result<Vec<String>> {
            let bridge = SyncIoBridge::new(reader);
            let decoder = GzDecoder::new(bridge);
            let mut archive = tar::Archive::new(decoder);
            let mut out = Vec::new();

            let entries = archive
                .entries()
                .map_err(|err| Error::Transcription(format!("cannot read the layer: {err}")))?;

            for entry in entries {
                let mut entry = entry.map_err(|err| {
                    Error::Transcription(format!("cannot read a layer entry: {err}"))
                })?;

                if !entry.header().entry_type().is_file() {
                    continue;
                }

                let path = entry.path().map_err(|err| {
                    Error::Transcription(format!("cannot read an entry path: {err}"))
                })?;

                let Some(relative) = path.strip_prefix(MODELS_PREFIX).ok().map(PathBuf::from)
                else {
                    continue;
                };

                let name = relative.to_string_lossy().to_string();
                if skip.iter().any(|prefix| name.starts_with(prefix.as_str())) {
                    continue;
                }

                let target = dest.join(&relative);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).map_err(|err| {
                        Error::Config(format!("cannot create {}: {err}", parent.display()))
                    })?;
                }

                let mut file = std::fs::File::create(&target).map_err(|err| {
                    Error::Config(format!("cannot write {}: {err}", target.display()))
                })?;

                std::io::copy(&mut entry, &mut file).map_err(|err| {
                    Error::Config(format!("cannot write {}: {err}", target.display()))
                })?;

                if let Some(sink) = &progress_for_task {
                    sink(Stage {
                        name: label.clone(),
                        bytes: base + counter_for_task.load(Ordering::Relaxed),
                        total,
                    });
                }

                out.push(name);
            }

            Ok(out)
        })
        .await
        .map_err(|err| Error::Transcription(format!("the extractor panicked: {err}")))??;

        written += counter.load(Ordering::Relaxed);
        files.extend(extracted);
    }

    // Swap the finished tree in. `dest` may be a corrupt leftover from an older
    // interrupted download, so it goes away first.
    let _ = std::fs::remove_dir_all(dest);
    if let Some(parent) = dest.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::rename(&staging, dest).map_err(|err| {
        Error::Config(format!(
            "cannot move {} into place: {err}",
            staging.display()
        ))
    })?;

    Ok(PulledImage {
        repository: repository.to_string(),
        digest,
        files,
        bytes: written,
    })
}

/// Asks the registry for an anonymous pull token.
async fn anonymous_token(client: &reqwest::Client, repository: &str) -> Result<String> {
    let url =
        format!("https://{REGISTRY}/token?scope=repository:{repository}:pull&service={REGISTRY}");

    let response = client
        .get(url)
        .send()
        .await
        .map_err(|err| Error::Transcription(format!("cannot reach {REGISTRY}: {err}")))?;

    if !response.status().is_success() {
        return Err(Error::Transcription(format!(
            "{REGISTRY} refused the token request for {repository}: {}",
            response.status()
        )));
    }

    #[derive(Deserialize)]
    struct Token {
        token: String,
    }

    let token: Token = response
        .json()
        .await
        .map_err(|err| Error::Transcription(format!("cannot read the token: {err}")))?;

    Ok(token.token)
}

/// Image manifest, after following an index when there is one.
struct Manifest {
    digest: String,
    layers: Vec<Layer>,
}

#[derive(Debug, Clone, Deserialize)]
struct Layer {
    #[serde(rename = "mediaType")]
    media_type: String,
    digest: String,
    size: u64,
}

#[derive(Deserialize)]
struct RawManifest {
    #[serde(default)]
    manifests: Vec<RawIndexEntry>,
    #[serde(default)]
    layers: Vec<Layer>,
}

#[derive(Deserialize)]
struct RawIndexEntry {
    digest: String,
    #[serde(default)]
    platform: Option<Platform>,
}

#[derive(Deserialize)]
struct Platform {
    architecture: String,
    os: String,
}

const ACCEPT: &str = "application/vnd.oci.image.index.v1+json, \
     application/vnd.docker.distribution.manifest.list.v2+json, \
     application/vnd.oci.image.manifest.v1+json, \
     application/vnd.docker.distribution.manifest.v2+json";

async fn fetch_manifest(
    client: &reqwest::Client,
    repository: &str,
    reference: &str,
    token: &str,
) -> Result<Manifest> {
    let raw = get_manifest(client, repository, reference, token).await?;

    // An index points at one manifest per platform. We ship linux/amd64.
    if !raw.manifests.is_empty() {
        let entry = raw
            .manifests
            .iter()
            .find(|entry| {
                entry.platform.as_ref().is_some_and(|platform| {
                    platform.os == "linux" && platform.architecture == "amd64"
                })
            })
            .ok_or_else(|| {
                Error::Transcription(format!("{repository}:{reference} has no linux/amd64 image"))
            })?;

        let resolved = get_manifest(client, repository, &entry.digest, token).await?;
        return Ok(Manifest {
            digest: entry.digest.clone(),
            layers: resolved
                .layers
                .into_iter()
                .filter(|layer| is_gzip_tar(&layer.media_type))
                .collect(),
        });
    }

    Ok(Manifest {
        digest: reference.to_string(),
        layers: raw
            .layers
            .into_iter()
            .filter(|layer| is_gzip_tar(&layer.media_type))
            .collect(),
    })
}

async fn get_manifest(
    client: &reqwest::Client,
    repository: &str,
    reference: &str,
    token: &str,
) -> Result<RawManifest> {
    let response = client
        .get(format!(
            "https://{REGISTRY}/v2/{repository}/manifests/{reference}"
        ))
        .bearer_auth(token)
        .header(reqwest::header::ACCEPT, ACCEPT)
        .send()
        .await
        .map_err(|err| Error::Transcription(format!("cannot read the manifest: {err}")))?;

    if !response.status().is_success() {
        return Err(Error::Transcription(format!(
            "cannot read the manifest of {repository}:{reference}: {}",
            response.status()
        )));
    }

    response
        .json()
        .await
        .map_err(|err| Error::Transcription(format!("unexpected manifest shape: {err}")))
}

/// Temporary directory used while a layer is being extracted.
fn staging_dir(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    dest.with_file_name(name)
}

fn blob_url(repository: &str, digest: &str) -> String {
    format!("https://{REGISTRY}/v2/{repository}/blobs/{digest}")
}

fn is_gzip_tar(media_type: &str) -> bool {
    media_type.ends_with("tar+gzip") || media_type.ends_with("tar.gzip")
}

/// Digest of the image a tag points at, resolved to our platform, without
/// pulling any layer.
///
/// This is the same digest `pull_models` records, so the number shown for a
/// model that is not installed yet is the one it will have once installed.
pub async fn remote_digest(
    client: &reqwest::Client,
    repository: &str,
    tag: &str,
) -> Result<String> {
    let token = anonymous_token(client, repository).await?;
    let manifest = fetch_manifest(client, repository, tag, &token).await?;

    Ok(manifest
        .digest
        .trim_start_matches("sha256:")
        .chars()
        .take(12)
        .collect())
}

/// File names a model image carries, from a pull, without downloading.
pub fn skip_full_attention() -> Vec<&'static str> {
    vec!["encoder-full"]
}

/// Reads the first bytes of a tar entry, used by tests.
pub fn read_prefix(mut reader: impl Read, bytes: usize) -> std::io::Result<Vec<u8>> {
    let mut buffer = vec![0u8; bytes];
    let read = reader.read(&mut buffer)?;
    buffer.truncate(read);
    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_gzip_tar_layers_are_considered() {
        assert!(is_gzip_tar("application/vnd.oci.image.layer.v1.tar+gzip"));
        assert!(is_gzip_tar(
            "application/vnd.docker.image.rootfs.diff.tar.gzip"
        ));
        assert!(!is_gzip_tar("application/vnd.oci.image.layer.v1.tar"));
        assert!(!is_gzip_tar("application/vnd.oci.image.layer.v1.tar+zstd"));
    }

    #[test]
    fn the_full_attention_encoder_is_skipped() {
        assert_eq!(skip_full_attention(), vec!["encoder-full"]);
    }

    #[test]
    fn blob_urls_point_at_the_registry() {
        assert_eq!(
            blob_url("achetronic/parakeet-models-asr-int8", "sha256:abc"),
            "https://ghcr.io/v2/achetronic/parakeet-models-asr-int8/blobs/sha256:abc"
        );
    }
}
