//! How the weights of the model reach this machine.
//!
//! The memory keeps the weights in `$EMBORNAL_HOME/models`. A file that is
//! already there is enough: the memory then asks the network nothing, which is
//! what makes a command that runs offline work.

use crate::config::EmbeddingConfig;
use crate::config::Paths;
use crate::error::{Error, Result};
use hf_hub::HFClientSync;
use hf_hub::progress::{DownloadEvent, Progress, ProgressEvent, ProgressHandler};
use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Returns the file of weights, and fetches it if it is absent.
pub fn ensure(config: &EmbeddingConfig, paths: &Paths) -> Result<PathBuf> {
    let file = config.weights_file(paths);
    if file.exists() {
        return Ok(file);
    }

    // A named file that is absent is a mistake of the configuration, not a
    // reason to download something else.
    if config.model_path.is_some() {
        return Err(Error::Io {
            path: file,
            source: std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "embedding.model_path names a file that is not there",
            ),
        });
    }

    let dir = paths.model_dir();
    std::fs::create_dir_all(&dir).map_err(|source| Error::Io {
        path: dir.clone(),
        source,
    })?;

    let (owner, name) = config.repo.split_once('/').ok_or_else(|| Error::Download {
        repo: config.repo.clone(),
        file: config.file.clone(),
        reason: "a repository has the owner/name form".to_string(),
    })?;

    let failed = |reason: String| Error::Download {
        repo: config.repo.clone(),
        file: config.file.clone(),
        reason,
    };

    eprintln!("embornal: fetching {} from {}", config.file, config.repo);
    let client = HFClientSync::new().map_err(|err| failed(err.to_string()))?;
    let downloaded = client
        .model(owner, name)
        .download_file()
        .filename(config.file.clone())
        .local_dir(dir)
        .progress(Progress::new(Bar::default()))
        .send()
        .map_err(|err| failed(err.to_string()))?;

    Ok(downloaded)
}

/// Writes the progress of the download.
///
/// It writes to the error stream, because the answer of a command goes to the
/// output stream and a pipe must not read this.
#[derive(Default)]
struct Bar {
    last: AtomicU64,
}

impl ProgressHandler for Bar {
    fn on_progress(&self, event: &ProgressEvent) {
        let ProgressEvent::Download(event) = event else {
            return;
        };
        match event {
            DownloadEvent::Progress { files } => {
                for file in files {
                    if file.total_bytes == 0 {
                        continue;
                    }
                    self.step(file.bytes_completed, file.total_bytes);
                }
            }
            DownloadEvent::AggregateProgress {
                bytes_completed,
                total_bytes,
                ..
            } => self.step(*bytes_completed, *total_bytes),
            DownloadEvent::Complete => {
                let mut err = std::io::stderr().lock();
                let _ = writeln!(err);
            }
            DownloadEvent::Start { .. } => {}
        }
    }
}

impl Bar {
    /// Draws one step, at most one for each whole percent.
    ///
    /// The events arrive many times each second. A line for each of them would
    /// bury whatever the command has to say.
    fn step(&self, done: u64, total: u64) {
        if total == 0 {
            return;
        }
        let percent = done.saturating_mul(100) / total;
        if self.last.swap(percent, Ordering::Relaxed) == percent {
            return;
        }

        let filled = (percent / 5) as usize;
        let mut err = std::io::stderr().lock();
        let _ = write!(
            err,
            "\rembornal: [{}{}] {} / {} MiB",
            "#".repeat(filled),
            "-".repeat(20 - filled),
            done / (1024 * 1024),
            total / (1024 * 1024),
        );
        let _ = err.flush();
    }
}
