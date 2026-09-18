//! Model download manager (§10): URL → models/<id>/model.gguf.
//!
//! Independent from inference: downloads never touch the loaded model.
//! Supports resume (HTTP Range + `.part` files), pause, cancel, and optional
//! SHA-256 verification. Progress is polled via the API; completion renames
//! `.part` → `model.gguf` and the UI re-scans (§9 layout).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadStatus {
    Downloading,
    Paused,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadInfo {
    pub id: String,
    pub url: String,
    pub dest: PathBuf,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub status: DownloadStatus,
    pub error: Option<String>,
    pub sha256: Option<String>,
}

#[derive(Debug)]
struct DownloadJob {
    info: DownloadInfo,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
}

#[derive(Debug, Default)]
pub struct DownloadManager {
    jobs: HashMap<String, DownloadJob>,
    models_dir: PathBuf,
}

impl DownloadManager {
    pub fn new(models_dir: PathBuf) -> Self {
        Self {
            jobs: HashMap::new(),
            models_dir,
        }
    }

    pub fn models_dir(&self) -> &Path {
        &self.models_dir
    }

    pub fn list(&self) -> Vec<DownloadInfo> {
        let mut v: Vec<_> = self
            .jobs
            .values()
            .map(|j| {
                let mut info = j.info.clone();
                info.downloaded_bytes = j
                    .info
                    .dest
                    .with_extension("gguf.part")
                    .metadata()
                    .map(|m| m.len())
                    .unwrap_or(j.info.downloaded_bytes);
                info
            })
            .collect();
        v.sort_by(|a, b| a.id.cmp(&b.id));
        v
    }

    pub fn get(&self, id: &str) -> Option<DownloadInfo> {
        self.jobs.get(id).map(|j| {
            let mut info = j.info.clone();
            info.downloaded_bytes = info
                .dest
                .with_extension("gguf.part")
                .metadata()
                .map(|m| m.len())
                .unwrap_or(info.downloaded_bytes);
            info
        })
    }

    /// Validate a new download request (§10: id, URL, destination).
    pub fn validate_request(id: &str, url: &str) -> Result<(), String> {
        if id.trim().is_empty() {
            return Err("model id is empty".into());
        }
        if !id
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.'))
        {
            return Err(format!(
                "invalid model id '{id}': use letters, numbers, '-', '_' or '.'"
            ));
        }
        if id == "." || id == ".." {
            return Err("invalid model id".into());
        }
        let url = url.trim();
        if !(url.starts_with("https://") || url.starts_with("http://")) {
            return Err(
                "URL must start with https:// or http:// (e.g. a HuggingFace resolve link)".into(),
            );
        }
        Ok(())
    }

    /// Start (or restart) a download. Spawns a background task; returns immediately.
    pub fn start(
        &mut self,
        id: String,
        url: String,
        sha256: Option<String>,
        http: reqwest::Client,
    ) -> Result<DownloadInfo, String> {
        Self::validate_request(&id, &url)?;
        if let Some(job) = self.jobs.get(&id) {
            if job.info.status == DownloadStatus::Downloading {
                return Err(format!("download '{id}' is already running"));
            }
        }
        let dir = self.models_dir.join(&id);
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let dest = dir.join("model.gguf");
        if dest.is_file() {
            return Err(format!(
                "'{id}' already has model.gguf. Delete the model first to re-download."
            ));
        }
        let info = DownloadInfo {
            id: id.clone(),
            url: url.clone(),
            dest: dest.clone(),
            total_bytes: None,
            downloaded_bytes: part_len(&dest),
            status: DownloadStatus::Downloading,
            error: None,
            sha256: sha256.filter(|s| !s.trim().is_empty()),
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        self.jobs.insert(
            id.clone(),
            DownloadJob {
                info: info.clone(),
                cancel: cancel.clone(),
                pause: pause.clone(),
            },
        );

        // The task owns everything it needs; progress is visible via the .part file size.
        tokio::spawn(download_task(
            id,
            url,
            dest,
            info.sha256.clone(),
            http,
            cancel,
            pause,
        ));
        Ok(info)
    }

    pub fn pause(&mut self, id: &str) -> Result<(), String> {
        match self.jobs.get(id) {
            Some(j) if j.info.status == DownloadStatus::Downloading => {
                j.pause.store(true, Ordering::SeqCst);
                Ok(())
            }
            Some(_) => Err(format!("download '{id}' is not running")),
            None => Err(format!("unknown download '{id}'")),
        }
    }

    pub fn cancel(&mut self, id: &str) -> Result<(), String> {
        match self.jobs.get(id) {
            Some(j) => {
                j.cancel.store(true, Ordering::SeqCst);
                Ok(())
            }
            None => Err(format!("unknown download '{id}'")),
        }
    }

    /// Resume a paused/failed/cancelled download from its `.part` offset.
    pub fn resume(&mut self, id: &str, http: reqwest::Client) -> Result<DownloadInfo, String> {
        let job = self
            .jobs
            .get(id)
            .ok_or_else(|| format!("unknown download '{id}'"))?;
        match job.info.status {
            DownloadStatus::Downloading | DownloadStatus::Verifying | DownloadStatus::Completed => {
                return Err(format!(
                    "download '{id}' is {:?}, nothing to resume",
                    job.info.status
                ));
            }
            _ => {}
        }
        if job.info.dest.is_file() {
            return Err(format!("'{id}' already has model.gguf"));
        }
        let url = job.info.url.clone();
        let dest = job.info.dest.clone();
        let sha256 = job.info.sha256.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        let job = self.jobs.get_mut(id).expect("checked");
        job.cancel = cancel.clone();
        job.pause = pause.clone();
        job.info.status = DownloadStatus::Downloading;
        job.info.error = None;
        let _ = FailedMarker::remove(&dest);
        tokio::spawn(download_task(
            id.to_string(),
            url,
            dest,
            sha256,
            http,
            cancel,
            pause,
        ));
        Ok(self.jobs.get(id).expect("checked").info.clone())
    }

    /// Called by the API layer to refresh a job's terminal state after the
    /// background task finishes (the task cannot borrow the manager).
    pub fn refresh(&mut self) {
        for job in self.jobs.values_mut() {
            let part = job.info.dest.with_extension("gguf.part");
            // Surface task-reported failures written via the sidecar marker.
            if job.info.status == DownloadStatus::Downloading {
                if let Some(msg) = FailedMarker::read(&job.info.dest) {
                    job.info.status = DownloadStatus::Failed;
                    job.info.error = Some(msg);
                    continue;
                }
            }
            let done = job.info.dest.is_file();
            if done && job.info.status == DownloadStatus::Downloading {
                job.info.status = DownloadStatus::Completed;
                job.info.downloaded_bytes = job.info.dest.metadata().map(|m| m.len()).unwrap_or(0);
            } else if !done && part.exists() {
                job.info.downloaded_bytes = part.metadata().map(|m| m.len()).unwrap_or(0);
                if job.cancel.load(Ordering::SeqCst) {
                    job.info.status = DownloadStatus::Cancelled;
                } else if job.pause.load(Ordering::SeqCst)
                    && job.info.status == DownloadStatus::Downloading
                {
                    job.info.status = DownloadStatus::Paused;
                }
            } else if !done && !part.exists() {
                if job.cancel.load(Ordering::SeqCst)
                    && job.info.status == DownloadStatus::Downloading
                {
                    job.info.status = DownloadStatus::Cancelled;
                }
            }
        }
    }

    pub fn mark_failed(&mut self, id: &str, error: String) {
        if let Some(job) = self.jobs.get_mut(id) {
            if job.info.status == DownloadStatus::Downloading {
                job.info.status = DownloadStatus::Failed;
                job.info.error = Some(error);
            }
        }
    }
}

fn part_len(dest: &Path) -> u64 {
    dest.with_extension("gguf.part")
        .metadata()
        .map(|m| m.len())
        .unwrap_or(0)
}

#[allow(clippy::too_many_arguments)]
async fn download_task(
    id: String,
    url: String,
    dest: PathBuf,
    sha256: Option<String>,
    http: reqwest::Client,
    cancel: Arc<AtomicBool>,
    pause: Arc<AtomicBool>,
) {
    use futures::StreamExt;
    use std::io::SeekFrom;
    use tokio::io::{AsyncSeekExt, AsyncWriteExt};

    let part = dest.with_extension("gguf.part");
    let resume_from = part.metadata().map(|m| m.len()).unwrap_or(0);
    let downloaded_counter = Arc::new(AtomicU64::new(resume_from));

    let mut req = http.get(&url);
    if resume_from > 0 {
        req = req.header("Range", format!("bytes={resume_from}-"));
    }
    let resp = match req.send().await {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("download {id} request failed: {e}");
            let _ = FailedMarker::write(&dest, &format!("request failed: {e}"));
            return;
        }
    };
    if !resp.status().is_success() && resp.status().as_u16() != 206 {
        let code = resp.status();
        tracing::warn!("download {id} HTTP {code}");
        let _ = FailedMarker::write(&dest, &format!("server returned {code}"));
        return;
    }
    let mut file = match tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .open(&part)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!("download {id} cannot write {}: {e}", part.display());
            return;
        }
    };
    if resume_from > 0 {
        // If the server ignored Range (200 instead of 206), restart from scratch.
        if resp.status().as_u16() == 200 {
            let _ = file.set_len(0).await;
            let _ = file.seek(SeekFrom::Start(0)).await;
            downloaded_counter.store(0, Ordering::SeqCst);
        } else if file.seek(SeekFrom::End(0)).await.is_err() {
            return;
        }
    }
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            tracing::info!("download {id} cancelled; removing partial file");
            drop(file);
            let _ = tokio::fs::remove_file(&part).await;
            let _ = FailedMarker::remove(&dest);
            return;
        }
        if pause.load(Ordering::SeqCst) {
            tracing::info!(
                "download {id} paused at {} bytes",
                downloaded_counter.load(Ordering::SeqCst)
            );
            return;
        }
        let bytes = match chunk {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!("download {id} stream error: {e}");
                let _ = FailedMarker::write(&dest, &format!("network error: {e}"));
                return;
            }
        };
        if file.write_all(&bytes).await.is_err() {
            let _ = FailedMarker::write(&dest, "disk write failed");
            return;
        }
        downloaded_counter.fetch_add(bytes.len() as u64, Ordering::SeqCst);
    }
    drop(file);

    if let Some(expected) = sha256 {
        if let Err(e) = verify_sha256(&part, &expected).await {
            tracing::warn!("download {id} checksum failed: {e}");
            let _ = FailedMarker::write(&dest, &format!("SHA-256 mismatch: {e}"));
            return;
        }
    }
    if tokio::fs::rename(&part, &dest).await.is_err() {
        let _ = FailedMarker::write(&dest, "cannot finalize file");
        return;
    }
    let _ = FailedMarker::remove(&dest);
    tracing::info!("download {id} complete: {}", dest.display());
}

/// Sidecar `<dest>.failed` marker so `refresh()` can surface errors without
/// shared mutable state with the spawned task.
struct FailedMarker;
impl FailedMarker {
    fn path(dest: &Path) -> PathBuf {
        dest.with_extension("failed")
    }
    fn write(dest: &Path, msg: &str) -> std::io::Result<()> {
        std::fs::write(Self::path(dest), msg)
    }
    fn remove(dest: &Path) -> std::io::Result<()> {
        let p = Self::path(dest);
        if p.exists() {
            std::fs::remove_file(p)
        } else {
            Ok(())
        }
    }
    pub fn read(dest: &Path) -> Option<String> {
        std::fs::read_to_string(Self::path(dest)).ok()
    }
}

async fn verify_sha256(path: &Path, expected: &str) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    use tokio::io::AsyncReadExt;
    let mut f = tokio::fs::File::open(path)
        .await
        .map_err(|e| format!("cannot open for verify: {e}"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f
            .read(&mut buf)
            .await
            .map_err(|e| format!("read failed: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let got = hex_encode(hasher.finalize());
    let expected = expected.trim().to_lowercase();
    if got == expected {
        Ok(())
    } else {
        Err(format!("expected {expected}, got {got}"))
    }
}

fn hex_encode(bytes: impl AsRef<[u8]>) -> String {
    bytes.as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Model size + resource estimates (§10, §14) for the detail endpoint.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ModelEstimates {
    pub file_gb: Option<f64>,
    pub gguf_present: bool,
    pub projector_present: bool,
    pub kv_cache_gb: f64,
    pub total_need_gb: Option<f64>,
    pub fits_vram: Option<bool>,
    pub fits_system: Option<bool>,
    pub compat_warnings: Vec<String>,
}

pub fn describe_model(
    meta: &crate::models::ModelMetadata,
    vram_gb: f64,
    ram_gb: f64,
) -> ModelEstimates {
    let gguf = meta.gguf_path();
    // Every part of a split model: the first part alone is its metadata.
    let file_bytes = Some(crate::models::model_set_bytes(&gguf)).filter(|bytes| *bytes > 0);
    let file_gb = file_bytes.map(|b| (b as f64) / 1_073_741_824.0);
    let gguf_present = gguf.is_file();
    let projector_present = meta
        .projector_file
        .as_ref()
        .map(|p| meta.dir.join(p).is_file())
        .unwrap_or(!meta.vision);
    let kv_cache_gb = (meta.context_length as f64) * 0.000_5;
    let total_need_gb = file_gb.map(|f| f + kv_cache_gb);
    let mut warnings = vec![];
    if !gguf_present {
        warnings.push(format!(
            "model.gguf missing in {}. Download or place the GGUF file.",
            meta.dir.display()
        ));
    }
    if meta.vision && !projector_present {
        warnings.push(
            "Vision model needs its projector (mmproj) file; set projector_file in metadata.json."
                .into(),
        );
    }
    let fits_vram = total_need_gb.map(|need| need <= vram_gb);
    let fits_system = total_need_gb.map(|need| need <= vram_gb + ram_gb);
    if let (Some(need), Some(false)) = (total_need_gb, fits_system) {
        warnings.push(format!(
            "Needs ~{need:.1} GB but only ~{:.1} GB (VRAM+RAM) is available. Use a smaller model or quantization.",
            vram_gb + ram_gb
        ));
    }
    ModelEstimates {
        file_gb: file_gb.map(|f| (f * 10.0).round() / 10.0),
        gguf_present,
        projector_present,
        kv_cache_gb: (kv_cache_gb * 10.0).round() / 10.0,
        total_need_gb: total_need_gb.map(|f| (f * 10.0).round() / 10.0),
        fits_vram,
        fits_system,
        compat_warnings: warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_ids_and_urls() {
        assert!(DownloadManager::validate_request("", "https://x/y").is_err());
        assert!(DownloadManager::validate_request("../evil", "https://x/y").is_err());
        assert!(DownloadManager::validate_request("ok-id_1", "ftp://x").is_err());
        assert!(DownloadManager::validate_request("example-14b", "https://hf.co/x.gguf").is_ok());
    }

    #[tokio::test]
    async fn duplicate_start_while_running_rejected() {
        let dir = std::env::temp_dir().join(format!("companion-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut m = DownloadManager::new(dir);
        let http = reqwest::Client::new();
        // Seed a fake running job via start with an existing part dir... use a
        // local opt-out: start twice; second must fail as already-running OR
        // as already-exists only after completion — here both are pending.
        let _ = m.start(
            "m1".into(),
            "https://example.com/m.gguf".into(),
            None,
            http.clone(),
        );
        let err = m
            .start("m1".into(), "https://example.com/m.gguf".into(), None, http)
            .unwrap_err();
        assert!(err.contains("already running"), "{err}");
    }

    #[tokio::test]
    async fn downloads_and_finalizes_small_file() {
        let body: Vec<u8> = (0..4096).map(|i| (i % 251) as u8).collect();
        let app = axum::Router::new().route(
            "/m.gguf",
            axum::routing::get({
                let body = body.clone();
                move || {
                    let body = body.clone();
                    async move { ([("content-length", "4096")], axum::body::Body::from(body)) }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let dir = std::env::temp_dir().join(format!("companion-dlok-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("t").join("model.gguf");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let pause = Arc::new(AtomicBool::new(false));
        download_task(
            "t".into(),
            format!("http://127.0.0.1:{port}/m.gguf"),
            dest.clone(),
            None,
            reqwest::Client::new(),
            cancel,
            pause,
        )
        .await;
        assert!(dest.is_file(), "final file should exist");
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), 4096);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn checksum_mismatch_blocks_finalize() {
        let app = axum::Router::new().route("/m.gguf", axum::routing::get(|| async { "hello" }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let dir = std::env::temp_dir().join(format!("companion-dlhash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dest = dir.join("t").join("model.gguf");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        download_task(
            "t".into(),
            format!("http://127.0.0.1:{port}/m.gguf"),
            dest.clone(),
            Some("deadbeef".into()),
            reqwest::Client::new(),
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
        )
        .await;
        assert!(!dest.is_file(), "bad checksum must not finalize");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
