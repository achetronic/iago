// SPDX-FileCopyrightText: 2026 Alby Hernández <hola@achetronic.com>
// SPDX-License-Identifier: Apache-2.0

//! What is actually installed under the models directory.
//!
//! The About page shows three models, and it must describe the files on disk,
//! not what we think is there. Each entry carries a short SHA-256 of its main
//! graph, which is the local equivalent of the model image digest: it identifies
//! the exact artifact without asking a registry.
//!
//! The pinned values below are copied from the Parakeet repository that exports
//! and downloads the assets. When the hash of `silero_vad.onnx` matches, the
//! version is confirmed; otherwise it says so instead of guessing.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use serde::{Deserialize, Serialize};

use crate::config::{Precision, Settings};
use crate::error::{Error, Result};
use crate::oci::{self, ProgressSink, PulledImage};
use crate::paths::Paths;

/// Silero VAD release pinned by Parakeet's Makefile.
pub const SILERO_VAD_VERSION: &str = "v6.2.1";
/// SHA-256 of that release, as pinned by Parakeet's Makefile.
pub const SILERO_VAD_SHA256: &str =
    "1a153a22f4509e292a94e67d6f9b85e8deb25b4988682b7e174c65279d8788e3";

/// Upstream identifier of the recognition checkpoint.
pub const PARAKEET_MODEL: &str = "nvidia/parakeet-tdt-0.6b-v3";
/// Upstream identifier of the diarization checkpoint.
pub const DIARIZATION_MODEL: &str = "nvidia/Nemotron-3-Diarization";
/// Commit the diarization checkpoint is pinned to.
pub const DIARIZATION_REVISION: &str = "f667ed73aee57d40cc39428eb768b4fd87a0a29e";
/// Where the VAD graph is published. There is no OCI image for it: upstream
/// publishes the ONNX directly, so Iago downloads it from the same place the
/// Parakeet Makefile does and checks the pinned hash.
pub const SILERO_VAD_URL: &str =
    "https://github.com/snakers4/silero-vad/raw/v6.2.1/src/silero_vad/data/silero_vad.onnx";
/// Repository holding the recognition models.
pub const ASR_REPOSITORY: &str = "achetronic/parakeet-models-asr";
/// Repository holding the diarization models.
pub const DIARIZATION_REPOSITORY: &str = "achetronic/parakeet-models-diarization";

/// One model, as found on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelInfo {
    /// What the model does, in two words.
    pub role: &'static str,
    /// Human name.
    pub name: &'static str,
    /// Who publishes the model.
    pub provider: &'static str,
    /// Upstream repository.
    pub upstream: &'static str,
    /// Cache key: `asr`, `vad` or `diarization`.
    pub component: &'static str,
    /// Image the model comes from. `None` for the VAD, which has no image.
    pub repository: Option<String>,
    /// Version string, confirmed when we can check it.
    pub version: String,
    /// Short SHA-256 of the main graph, or `None` when it is not installed.
    pub digest: Option<String>,
    /// Digest of the image this model came from, when we pulled it ourselves.
    pub image_digest: Option<String>,
    /// Total size of the files that are present, in megabytes.
    pub size_mb: u64,
    /// Files the model needs that are missing.
    pub missing: Vec<String>,
}

impl ModelInfo {
    /// `true` when every file the model needs is present.
    pub fn installed(&self) -> bool {
        self.missing.is_empty()
    }

    /// Short digest, or the reason there is none.
    pub fn digest_label(&self) -> String {
        match &self.image_digest {
            // The image this model came from, or the one a download would fetch.
            Some(digest) => digest.clone(),
            None if self.installed() => "image digest unknown".to_string(),
            None => "not installed".to_string(),
        }
    }
}

/// Model root, honouring the configured override.
pub fn root(settings: &Settings, paths: &Paths) -> PathBuf {
    if settings.asr.models_dir.is_empty() {
        paths.models_dir()
    } else {
        PathBuf::from(&settings.asr.models_dir)
    }
}

/// What an install did.
#[derive(Debug, Clone)]
pub struct InstallReport {
    /// Directory the models were written to.
    pub root: PathBuf,
    /// Images pulled, with their digests.
    pub images: Vec<PulledImage>,
    /// SHA-256 of the VAD graph that was written.
    pub vad_digest: String,
    /// Models found after installing.
    pub models: Vec<ModelInfo>,
}

/// Downloads every model the app needs, replacing whatever was there.
///
/// It always downloads. Re-running is the repair path for a corrupt or partial
/// install, and it is also how a new model release lands: the tag is `latest`,
/// so the layers change under it.
pub async fn install(
    settings: &Settings,
    paths: &Paths,
    progress: Option<ProgressSink>,
) -> Result<InstallReport> {
    let root = root(settings, paths);
    let precision = settings.asr.precision;

    let mut images = Vec::new();
    let asr = install_asr(precision, &root, progress.clone()).await?;
    let vad_digest = install_vad(&root, progress.clone()).await?;
    let diarization = install_diarization(precision, &root, progress).await?;

    let mut cache = ImageCache::load(paths);
    cache.record("asr", &asr.repository, &asr.digest);
    cache.record("vad", "snakers4/silero-vad", &vad_digest);
    cache.record("diarization", &diarization.repository, &diarization.digest);
    cache.save(paths)?;

    images.push(asr);
    images.push(diarization);

    Ok(InstallReport {
        models: inspect_with_images(&root, precision, paths),
        root,
        images,
        vad_digest,
    })
}

/// Pulls the recognition models for one precision.
pub async fn install_asr(
    precision: Precision,
    root: &Path,
    progress: Option<ProgressSink>,
) -> Result<PulledImage> {
    let repository = format!("{ASR_REPOSITORY}-{}", precision.as_str());
    let client = download_client()?;

    oci::pull_models(
        &client,
        &repository,
        oci::TAG,
        &root.join("asr").join(precision.as_str()),
        &format!("asr/{}", precision.as_str()),
        &oci::skip_full_attention(),
        progress,
    )
    .await
}

/// Pulls the diarization models for one precision.
pub async fn install_diarization(
    precision: Precision,
    root: &Path,
    progress: Option<ProgressSink>,
) -> Result<PulledImage> {
    let repository = format!("{DIARIZATION_REPOSITORY}-{}", precision.as_str());
    let client = download_client()?;

    oci::pull_models(
        &client,
        &repository,
        oci::TAG,
        &root.join("diarization").join(precision.as_str()),
        &format!("diarization/{}", precision.as_str()),
        &[],
        progress,
    )
    .await
}

/// Downloads the VAD graph and verifies its pinned hash.
pub async fn install_vad(root: &Path, progress: Option<ProgressSink>) -> Result<String> {
    let client = download_client()?;
    let destination = root.join("vad").join("silero_vad.onnx");

    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|err| Error::Config(format!("cannot create {}: {err}", parent.display())))?;
    }

    let response = client
        .get(SILERO_VAD_URL)
        .send()
        .await
        .map_err(|err| Error::Transcription(format!("cannot download the VAD model: {err}")))?;

    if !response.status().is_success() {
        return Err(Error::Transcription(format!(
            "cannot download the VAD model: {}",
            response.status()
        )));
    }

    let total = response.content_length().unwrap_or(0);
    let mut file = std::fs::File::create(&destination)
        .map_err(|err| Error::Config(format!("cannot write {}: {err}", destination.display())))?;

    let mut hasher = Sha256::new();
    let mut written = 0u64;
    let mut stream = response.bytes_stream();

    use futures_util::StreamExt as _;
    use std::io::Write as _;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk
            .map_err(|err| Error::Transcription(format!("the VAD download stopped: {err}")))?;
        hasher.update(&chunk);
        file.write_all(&chunk).map_err(|err| {
            Error::Config(format!("cannot write {}: {err}", destination.display()))
        })?;
        written += chunk.len() as u64;

        if let Some(sink) = &progress {
            sink(oci::Stage {
                name: "vad".to_string(),
                bytes: written,
                total,
            });
        }
    }

    let digest: String = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    if digest != SILERO_VAD_SHA256 {
        let _ = std::fs::remove_file(&destination);
        return Err(Error::Transcription(format!(
            "the VAD model does not match the pinned hash: got {}, expected {SILERO_VAD_SHA256}",
            &digest[..12]
        )));
    }

    Ok(digest)
}

/// HTTP client for large downloads: no global timeout, because a 875 MB layer
/// on a home connection takes longer than any sensible request timeout.
fn download_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|err| Error::Transcription(format!("cannot build the HTTP client: {err}")))
}

/// Cache of the images last pulled, stored next to the settings file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImageCache {
    /// Component (`asr`, `vad`, `diarization`) to what was pulled.
    #[serde(default)]
    pub images: std::collections::BTreeMap<String, CachedImage>,
}

/// One pulled image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedImage {
    /// Repository it came from, or the upstream project for the VAD.
    pub repository: String,
    /// Manifest digest, or the artifact hash when there is no image.
    pub digest: String,
    /// Unix time of the pull.
    pub pulled_at: u64,
}

impl ImageCache {
    /// Reads the cache next to the settings, or an empty one.
    pub fn load(paths: &Paths) -> Self {
        std::fs::read_to_string(paths.images_file())
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_default()
    }

    /// Writes the cache, creating the directory when needed.
    pub fn save(&self, paths: &Paths) -> Result<()> {
        let path = paths.images_file();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self)?)?;
        Ok(())
    }

    /// Digest recorded for a component, shortened for display.
    pub fn short_digest(&self, component: &str) -> Option<String> {
        self.images.get(component).map(|entry| {
            entry
                .digest
                .trim_start_matches("sha256:")
                .chars()
                .take(12)
                .collect()
        })
    }

    /// Records a component.
    pub fn record(&mut self, component: &str, repository: &str, digest: &str) {
        self.images.insert(
            component.to_string(),
            CachedImage {
                repository: repository.to_string(),
                digest: digest.to_string(),
                pulled_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|elapsed| elapsed.as_secs())
                    .unwrap_or(0),
            },
        );
    }
}

/// Inspects the three models under `root`.
pub fn inspect(root: &Path, precision: Precision) -> Vec<ModelInfo> {
    vec![
        parakeet(root, precision),
        vad(root),
        diarization(root, precision),
    ]
}

/// Fills the digest of models that are not installed yet with the digest of the
/// image a download would fetch.
///
/// Network failures are ignored on purpose: the page then shows
/// `image digest unknown` instead of breaking.
pub async fn fill_remote_digests(models: &mut [ModelInfo]) {
    let Ok(client) = download_client() else {
        return;
    };

    for info in models.iter_mut() {
        if info.installed() || info.image_digest.is_some() {
            continue;
        }

        match &info.repository {
            Some(repository) => {
                if let Ok(digest) = oci::remote_digest(&client, repository, oci::TAG).await {
                    info.image_digest = Some(digest);
                }
            }
            // The VAD has no image: its artifact hash is the pinned one.
            None => {
                info.image_digest = Some(SILERO_VAD_SHA256.chars().take(12).collect());
            }
        }
    }
}

/// Inspects the models and attaches the image digests recorded at install time.
pub fn inspect_with_images(root: &Path, precision: Precision, paths: &Paths) -> Vec<ModelInfo> {
    let cache = ImageCache::load(paths);
    let mut models = inspect(root, precision);

    for info in &mut models {
        info.image_digest = cache.short_digest(info.component);
    }

    models
}

fn parakeet(root: &Path, precision: Precision) -> ModelInfo {
    let (encoder, decoder) = match precision {
        Precision::Int8 => ("encoder-local.int8.onnx", "decoder_joint-model.int8.onnx"),
        Precision::Fp32 => ("encoder-local.onnx", "decoder_joint-model.onnx"),
    };

    let dir = root.join("asr").join(precision.as_str());

    model(
        ModelInfo {
            role: "Transcription",
            name: "Parakeet TDT 0.6B",
            provider: "NVIDIA",
            upstream: PARAKEET_MODEL,
            component: "asr",
            repository: Some(format!("{ASR_REPOSITORY}-{}", precision.as_str())),
            version: format!("v3 · {}", precision.as_str()),
            digest: None,
            image_digest: None,
            size_mb: 0,
            missing: Vec::new(),
        },
        &dir,
        &[encoder, decoder],
    )
}

fn vad(root: &Path) -> ModelInfo {
    let mut info = model(
        ModelInfo {
            role: "Activity Detector",
            name: "Silero VAD",
            provider: "Silero",
            upstream: "snakers4/silero-vad",
            component: "vad",
            repository: None,
            version: SILERO_VAD_VERSION.to_string(),
            digest: None,
            image_digest: None,
            size_mb: 0,
            missing: Vec::new(),
        },
        &root.join("vad"),
        &["silero_vad.onnx"],
    );

    // The version is only "confirmed" when the bytes match the pin.
    if info.digest.is_some() {
        let matches = full_digest(&root.join("vad").join("silero_vad.onnx"))
            .is_some_and(|full| full == SILERO_VAD_SHA256);
        info.version = if matches {
            format!("{SILERO_VAD_VERSION} (pinned hash matches)")
        } else {
            format!("{SILERO_VAD_VERSION} (hash differs from the pin)")
        };
    }

    info
}

fn diarization(root: &Path, precision: Precision) -> ModelInfo {
    let file = match precision {
        Precision::Int8 => "model_quantized.onnx",
        Precision::Fp32 => "model.onnx",
    };

    model(
        ModelInfo {
            role: "Diarization",
            name: "Nemotron-3 Diarization",
            provider: "NVIDIA",
            upstream: DIARIZATION_MODEL,
            component: "diarization",
            repository: Some(format!("{DIARIZATION_REPOSITORY}-{}", precision.as_str())),
            version: format!("{} · {}", &DIARIZATION_REVISION[..8], precision.as_str()),
            digest: None,
            image_digest: None,
            size_mb: 0,
            missing: Vec::new(),
        },
        &root.join("diarization").join(precision.as_str()),
        &[file, "config.json"],
    )
}

/// Fills size, digest and missing files for one model.
///
/// The graph and its external weights are two files: the quantised diarization
/// graph is 364 KB and its `_data` companion is 120 MB, so counting only the
/// `.onnx` would report a model that is not there.
fn model(mut info: ModelInfo, dir: &Path, required: &[&str]) -> ModelInfo {
    for name in required {
        let path = dir.join(name);
        if !path.is_file() {
            info.missing.push((*name).to_string());
            continue;
        }
        info.size_mb += size_mb(&path);

        for companion in [format!("{name}_data"), format!("{name}.data")] {
            let companion = dir.join(companion);
            if companion.is_file() {
                info.size_mb += size_mb(&companion);
            }
        }
    }

    if info.missing.is_empty() {
        info.digest = digest(&dir.join(required[0]));
    }

    info
}

/// Short SHA-256 of a file, or `None` when it cannot be read.
pub fn digest(path: &Path) -> Option<String> {
    let full = full_digest(path)?;
    Some(full[..12].to_string())
}

/// Size of a file in megabytes.
pub fn size_mb(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|meta| meta.len() / (1024 * 1024))
        .unwrap_or(0)
}

/// Full SHA-256 of a file, in lowercase hex.
fn full_digest(path: &Path) -> Option<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path).ok()?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];

    loop {
        let read = file.read(&mut buffer).ok()?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let digest = hasher.finalize();
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn a_missing_model_is_reported_as_such() {
        let dir = tempfile::tempdir().expect("tempdir");
        let models = inspect(dir.path(), Precision::Int8);

        assert_eq!(models.len(), 3);
        for info in &models {
            assert!(!info.installed());
            assert_eq!(info.digest_label(), "not installed");
            assert!(!info.missing.is_empty());
        }
    }

    #[test]
    fn an_installed_model_carries_a_short_file_digest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let vad = dir.path().join("vad");
        std::fs::create_dir_all(&vad).expect("vad dir");

        let mut file = std::fs::File::create(vad.join("silero_vad.onnx")).expect("file");
        file.write_all(b"not a real model").expect("write");

        let models = inspect(dir.path(), Precision::Int8);
        let silero = models.iter().find(|m| m.name == "Silero VAD").expect("vad");

        assert!(silero.installed());
        assert_eq!(silero.provider, "Silero");

        let digest = silero.digest.as_deref().expect("digest");
        assert_eq!(digest.len(), 12);
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(silero.version.contains("hash differs from the pin"));

        // Without a recorded image, the label says so instead of inventing one.
        assert_eq!(silero.digest_label(), "image digest unknown");
    }

    #[test]
    fn a_missing_model_shows_the_digest_of_what_would_be_installed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let paths = Paths::with_root(dir.path().join("root"));

        let mut models = inspect(dir.path(), Precision::Int8);
        models[0].image_digest = Some("6e12d5e4cafe".to_string());

        assert!(!models[0].installed());
        assert_eq!(models[0].digest_label(), "6e12d5e4cafe");
        assert_eq!(models[1].digest_label(), "not installed");

        // And the cache is what feeds the installed case.
        let mut cache = ImageCache::load(&paths);
        cache.record(
            "asr",
            "achetronic/parakeet-models-asr-int8",
            "sha256:abc123456789",
        );
        assert_eq!(cache.short_digest("asr").as_deref(), Some("abc123456789"));
    }
}
