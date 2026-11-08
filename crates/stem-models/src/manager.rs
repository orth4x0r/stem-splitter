use std::path::PathBuf;
use std::sync::atomic::AtomicBool;

use crate::download::{DownloadStatus, download_with_status, part_path};
use crate::{ModelInfo, ModelManager, Progress, Result, runtime};

impl ModelManager {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// `<root>/models`.
    pub fn models_dir(&self) -> PathBuf {
        self.root.join("models")
    }

    /// `<root>/runtime`, where onnxruntime (and DirectML.dll on Windows) live.
    pub fn runtime_dir(&self) -> PathBuf {
        runtime::runtime_dir(&self.root)
    }

    pub fn model_path(&self, info: &ModelInfo) -> PathBuf {
        self.models_dir().join(info.file_name())
    }

    pub fn is_downloaded(&self, info: &ModelInfo) -> bool {
        self.model_path(info).is_file()
    }

    /// Download `info` to [`ModelManager::model_path`] (via a `.part` file, verified against
    /// `info.sha256` when set). Returns immediately if the file is already there.
    pub fn download(
        &self,
        info: &ModelInfo,
        progress: &mut dyn FnMut(Progress),
        cancel: &AtomicBool,
    ) -> Result<PathBuf> {
        self.download_with_status(
            info,
            &mut |s| {
                progress(Progress::Downloading {
                    fraction: s.fraction(),
                })
            },
            cancel,
        )
    }

    /// [`ModelManager::download`] reporting bytes, total and speed. A download that was
    /// cancelled or interrupted earlier resumes from its `.part` file (HTTP Range).
    pub fn download_with_status(
        &self,
        info: &ModelInfo,
        progress: &mut dyn FnMut(&DownloadStatus),
        cancel: &AtomicBool,
    ) -> Result<PathBuf> {
        let dest = self.model_path(info);
        if dest.is_file() {
            return Ok(dest);
        }
        download_with_status(info.url, &dest, info.sha256, progress, cancel)?;
        Ok(dest)
    }

    /// Bytes of an unfinished download of `info` waiting to be resumed (0 if none).
    pub fn partial_bytes(&self, info: &ModelInfo) -> u64 {
        part_path(&self.model_path(info))
            .metadata()
            .map_or(0, |m| m.len())
    }

    /// Make sure onnxruntime (DirectML build on Windows) is present and initialise `ort` with it.
    /// Safe to call repeatedly.
    pub fn ensure_runtime(&self, progress: &mut dyn FnMut(Progress)) -> Result<()> {
        runtime::ensure(&self.root, progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::find_model;

    #[test]
    fn paths() {
        let m = ModelManager::new(PathBuf::from("/data"));
        let info = find_model("mdx-kim-vocal-2").unwrap();
        assert_eq!(
            m.model_path(&info),
            PathBuf::from("/data/models/Kim_Vocal_2.onnx")
        );
        assert!(!m.is_downloaded(&info));
    }

    #[test]
    fn cancelled_download_leaves_nothing() {
        let dir = std::env::temp_dir().join(format!("stem-models-dl-{}", std::process::id()));
        let m = ModelManager::new(dir.clone());
        let info = find_model("mdx-kim-vocal-2").unwrap();
        let cancel = AtomicBool::new(true);
        // Either the request fails (no network) or the first cancel check stops it.
        let r = m.download(&info, &mut |_| {}, &cancel);
        assert!(r.is_err());
        assert!(!m.model_path(&info).exists());
        assert!(!crate::download::part_path(&m.model_path(&info)).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
