//! Locating (and on Windows downloading) the ONNX Runtime library, initialising `ort`, and
//! building sessions on a [`Device`].
//!
//! `ort` 2.0.0-rc.15 is compiled with `api-22`, so it needs ONNX Runtime >= 1.22 (it refuses
//! older libraries at load time). On Windows we fetch the official DirectML build from NuGet:
//!
//! * `Microsoft.ML.OnnxRuntime.DirectML` [`ORT_VERSION`] -> `runtimes/win-x64/native/onnxruntime.dll`
//! * `Microsoft.AI.DirectML` [`DIRECTML_VERSION`] (the version that ORT package depends on)
//!   -> `bin/x64-win/DirectML.dll`
//!
//! Both land in `<root>/runtime/`. `DirectML.dll` is loaded by full path *before*
//! `onnxruntime.dll`, so when onnxruntime asks the loader for `DirectML.dll` it gets the
//! already-loaded copy instead of the much older one in System32.
//!
//! Elsewhere (and on Windows too, as a developer override) `ORT_DYLIB_PATH` names the library;
//! otherwise `<root>/runtime/libonnxruntime.so` (`.dylib` on macOS) is used.

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use ort::environment::Environment;
use ort::session::Session;

use crate::network::OrtNetwork;
use crate::{Device, LoadOptions, ModelError, Progress, Result, devices};

/// ONNX Runtime version downloaded on Windows (DirectML build).
pub const ORT_VERSION: &str = "1.24.4";
/// DirectML redistributable version downloaded next to it (what ORT 1.24.4 depends on).
pub const DIRECTML_VERSION: &str = "1.15.4";

static ENV: OnceLock<Environment> = OnceLock::new();
static INIT: Mutex<()> = Mutex::new(());

fn rt_err(e: impl std::fmt::Display) -> ModelError {
    ModelError::Runtime(e.to_string())
}

pub(crate) fn runtime_dir(root: &Path) -> PathBuf {
    root.join("runtime")
}

pub(crate) fn library_file_name() -> String {
    format!(
        "{}onnxruntime{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
}

fn env_override() -> Option<PathBuf> {
    std::env::var_os("ORT_DYLIB_PATH")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// See [`crate::ModelManager::ensure_runtime`].
pub(crate) fn ensure(root: &Path, progress: &mut dyn FnMut(Progress)) -> Result<()> {
    if ENV.get().is_some() {
        return Ok(());
    }
    let _guard = INIT.lock().unwrap_or_else(|e| e.into_inner());
    if ENV.get().is_some() {
        return Ok(());
    }
    if let Some(path) = env_override() {
        return init_from(&path);
    }
    let dir = runtime_dir(root);
    let lib = dir.join(library_file_name());
    #[cfg(windows)]
    if !lib.is_file() || !dir.join("DirectML.dll").is_file() {
        nuget::fetch_windows_runtime(&dir, progress)?;
    }
    #[cfg(not(windows))]
    let _ = &progress;
    if !lib.is_file() {
        return Err(ModelError::Runtime(format!(
            "ONNX Runtime not found: set ORT_DYLIB_PATH or put {} (>= 1.22) in {}",
            library_file_name(),
            dir.display()
        )));
    }
    init_from(&lib)
}

fn init_from(lib: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let dml = lib.with_file_name("DirectML.dll");
        if dml.is_file() {
            ort::util::preload_dylib(&dml)
                .map_err(|e| ModelError::Runtime(format!("loading {}: {e}", dml.display())))?;
        } else {
            tracing::warn!(
                "no DirectML.dll next to {}; DirectML may use the outdated System32 copy",
                lib.display()
            );
        }
    }
    let env = ort::init_from(lib)
        .map_err(rt_err)?
        .with_name("stem-splitter")
        .with_telemetry(false)
        .build()
        .map_err(rt_err)?;
    tracing::info!("ONNX Runtime loaded from {}", lib.display());
    let _ = ENV.set(env);
    Ok(())
}

/// The initialised environment. Falls back to `ORT_DYLIB_PATH` if `ensure_runtime` was never
/// called (handy in tests).
pub(crate) fn environment() -> Result<Environment> {
    if let Some(e) = ENV.get() {
        return Ok(e.clone());
    }
    let _guard = INIT.lock().unwrap_or_else(|e| e.into_inner());
    if ENV.get().is_none() {
        match env_override() {
            Some(p) => init_from(&p)?,
            None => {
                return Err(ModelError::Runtime(
                    "ONNX Runtime is not initialised: call ModelManager::ensure_runtime first"
                        .into(),
                ));
            }
        }
    }
    ENV.get()
        .cloned()
        .ok_or_else(|| ModelError::Runtime("ONNX Runtime failed to initialise".into()))
}

/// Symbolic batch dimension name used by UVR's MDX exports and by our HTDemucs export.
const BATCH_DIM_NAMES: &[&str] = &["batch_size", "batch"];

/// Build a session for `model_path` on `device`, falling back to the CPU (with a warning) if
/// DirectML cannot be initialised.
pub(crate) fn build_session(
    model_path: &Path,
    device: Device,
    options: &LoadOptions,
    batch: usize,
) -> Result<OrtNetwork> {
    let env = environment()?;
    let resolved = devices::resolve(device);
    if let Device::DirectMl(id) = resolved {
        match directml_session(&env, model_path, id, batch) {
            Ok(session) => {
                return Ok(OrtNetwork {
                    session,
                    device: resolved,
                });
            }
            Err(e) => tracing::warn!("DirectML adapter {id} unavailable ({e}); using the CPU"),
        }
    }
    let mut b = Session::builder(&env).map_err(rt_err)?;
    if let Some(n) = options.cpu_threads {
        b = b.with_intra_threads(n).map_err(rt_err)?;
    }
    let session = b.commit_from_file(model_path).map_err(rt_err)?;
    Ok(OrtNetwork {
        session,
        device: Device::Cpu,
    })
}

fn directml_session(
    env: &Environment,
    model_path: &Path,
    id: u32,
    batch: usize,
) -> Result<Session> {
    use ort::ep;
    // The DirectML EP does not support memory patterns or parallel execution.
    let mut b = Session::builder(env)
        .map_err(rt_err)?
        .with_memory_pattern(false)
        .map_err(rt_err)?
        .with_parallel_execution(false)
        .map_err(rt_err)?;
    // DirectML plans much better with static shapes: pin the batch dimension. Unknown names
    // are ignored by onnxruntime.
    for name in BATCH_DIM_NAMES {
        b = b
            .with_dimension_override(*name, batch as i64)
            .map_err(rt_err)?;
    }
    b.with_execution_providers([ep::DirectML::default()
        .with_device_id(id as i32)
        .build()
        .error_on_failure()])
        .map_err(rt_err)?
        .commit_from_file(model_path)
        .map_err(rt_err)
}

/// Downloading the DirectML build of onnxruntime from NuGet.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) mod nuget {
    use std::fs::File;
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use std::time::Duration;

    use super::{DIRECTML_VERSION, ORT_VERSION};
    use crate::download::{download_to, part_path, sha256_file};
    use crate::{ModelError, Progress, Result};

    pub(crate) struct NativeFile {
        pub package: &'static str,
        pub version: &'static str,
        /// Path inside the .nupkg (a zip).
        pub entry: &'static str,
        pub file: &'static str,
        pub sha256: &'static str,
    }

    /// Order matters: onnxruntime.dll last, its presence marks a complete install.
    pub(crate) const FILES: &[NativeFile] = &[
        NativeFile {
            package: "Microsoft.AI.DirectML",
            version: DIRECTML_VERSION,
            entry: "bin/x64-win/DirectML.dll",
            file: "DirectML.dll",
            sha256: "9c9e6d822561c6c41b90e6994b3e8857cf1d66dbfb1e0c4c799c7c89b4e92da1",
        },
        NativeFile {
            package: "Microsoft.ML.OnnxRuntime.DirectML",
            version: ORT_VERSION,
            entry: "runtimes/win-x64/native/onnxruntime.dll",
            file: "onnxruntime.dll",
            sha256: "e7eedec6a6f26dc39dc948276a75ef6d2bee3fff944d874ceed0bbd3b97bff40",
        },
    ];

    pub(crate) fn package_urls(package: &str, version: &str) -> [String; 2] {
        let lower = package.to_ascii_lowercase();
        [
            format!(
                "https://api.nuget.org/v3-flatcontainer/{lower}/{version}/{lower}.{version}.nupkg"
            ),
            format!("https://www.nuget.org/api/v2/package/{package}/{version}"),
        ]
    }

    pub(crate) fn fetch_windows_runtime(
        dir: &Path,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<()> {
        std::fs::create_dir_all(dir)?;
        let n = FILES.len() as f32;
        for (i, f) in FILES.iter().enumerate() {
            let dest = dir.join(f.file);
            if dest.is_file() && sha256_file(&dest)?.eq_ignore_ascii_case(f.sha256) {
                continue;
            }
            let mut sub = |x: Option<f32>| {
                progress(Progress::Downloading {
                    fraction: x.map(|x| (i as f32 + x) / n),
                })
            };
            fetch_file(f, &dest, &mut sub)?;
        }
        Ok(())
    }

    fn fetch_file(
        f: &NativeFile,
        dest: &Path,
        progress: &mut dyn FnMut(Option<f32>),
    ) -> Result<()> {
        let mut last_err = None;
        for url in package_urls(f.package, f.version) {
            // The DirectML package is ~200 MB for an 18 MB DLL, so read just that entry with
            // HTTP range requests; fall back to downloading the whole package.
            match RangeReader::open(&url) {
                Ok(Some(reader)) => match extract(reader, f, dest, progress) {
                    Ok(()) => return Ok(()),
                    Err(e) => tracing::warn!("range download of {url} failed: {e}"),
                },
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!("{url}: {e}");
                    last_err = Some(e);
                    continue;
                }
            }
            let pkg = dest.with_file_name(format!("{}.{}.nupkg", f.package, f.version));
            let never = AtomicBool::new(false);
            match download_to(&url, &pkg, None, progress, &never) {
                Ok(()) => {
                    let r = File::open(&pkg)
                        .map_err(ModelError::from)
                        .and_then(|file| extract(file, f, dest, &mut |_| {}));
                    let _ = std::fs::remove_file(&pkg);
                    match r {
                        Ok(()) => return Ok(()),
                        Err(e) => last_err = Some(e),
                    }
                }
                Err(e) => last_err = Some(e),
            }
        }
        Err(last_err.unwrap_or_else(|| ModelError::Download(format!("{}: no source", f.package))))
    }

    fn extract<R: Read + Seek>(
        reader: R,
        f: &NativeFile,
        dest: &Path,
        progress: &mut dyn FnMut(Option<f32>),
    ) -> Result<()> {
        let zip_err =
            |e: zip::result::ZipError| ModelError::Download(format!("{}: {e}", f.package));
        let mut archive = zip::ZipArchive::new(reader).map_err(zip_err)?;
        let mut entry = archive.by_name(f.entry).map_err(zip_err)?;
        let size = entry.size().max(1);
        let part = part_path(dest);
        let result = (|| -> Result<()> {
            let mut out = File::create(&part)?;
            let mut buf = vec![0u8; 256 * 1024];
            let mut done = 0u64;
            loop {
                let n = entry.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                out.write_all(&buf[..n])?;
                done += n as u64;
                progress(Some((done as f64 / size as f64).min(1.0) as f32));
            }
            out.sync_all()?;
            Ok(())
        })();
        if let Err(e) = result {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
        let got = sha256_file(&part)?;
        if !got.eq_ignore_ascii_case(f.sha256) {
            let _ = std::fs::remove_file(&part);
            return Err(ModelError::Download(format!(
                "{} from {} {}: sha256 mismatch (expected {}, got {got})",
                f.file, f.package, f.version, f.sha256
            )));
        }
        std::fs::rename(&part, dest)?;
        Ok(())
    }

    /// `Read + Seek` over an HTTP resource using `Range` requests, one 1 MiB block at a time.
    pub(crate) struct RangeReader {
        client: reqwest::blocking::Client,
        url: String,
        len: u64,
        pos: u64,
        block_start: u64,
        block: Vec<u8>,
    }

    const BLOCK: u64 = 1 << 20;

    impl RangeReader {
        /// `Ok(None)` if the server ignores `Range`.
        pub(crate) fn open(url: &str) -> Result<Option<Self>> {
            let client = reqwest::blocking::Client::builder()
                .user_agent(concat!("stem-splitter/", env!("CARGO_PKG_VERSION")))
                .connect_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(120))
                .build()
                .map_err(|e| ModelError::Download(e.to_string()))?;
            let resp = client
                .get(url)
                .header(reqwest::header::RANGE, "bytes=0-0")
                .send()
                .map_err(|e| ModelError::Download(format!("{url}: {e}")))?;
            if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                if resp.status().is_success() {
                    return Ok(None);
                }
                return Err(ModelError::Download(format!(
                    "{url}: HTTP {}",
                    resp.status()
                )));
            }
            let len = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.rsplit('/').next())
                .and_then(|v| v.trim().parse::<u64>().ok());
            let Some(len) = len else {
                return Ok(None);
            };
            Ok(Some(Self {
                client,
                url: url.to_string(),
                len,
                pos: 0,
                block_start: 0,
                block: Vec::new(),
            }))
        }

        fn fetch(&mut self, start: u64) -> std::io::Result<()> {
            let end = (start + BLOCK).min(self.len) - 1;
            let io = |e: String| std::io::Error::other(e);
            let mut resp = self
                .client
                .get(&self.url)
                .header(reqwest::header::RANGE, format!("bytes={start}-{end}"))
                .send()
                .map_err(|e| io(e.to_string()))?;
            if resp.status() != reqwest::StatusCode::PARTIAL_CONTENT {
                return Err(io(format!("HTTP {} for a range request", resp.status())));
            }
            self.block.clear();
            resp.read_to_end(&mut self.block)?;
            if self.block.len() as u64 != end - start + 1 {
                return Err(io("short range response".into()));
            }
            self.block_start = start;
            Ok(())
        }
    }

    impl Read for RangeReader {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.pos >= self.len || buf.is_empty() {
                return Ok(0);
            }
            let in_block = self.pos >= self.block_start
                && self.pos < self.block_start + self.block.len() as u64;
            if !in_block {
                self.fetch(self.pos)?;
            }
            let off = (self.pos - self.block_start) as usize;
            let n = buf.len().min(self.block.len() - off);
            buf[..n].copy_from_slice(&self.block[off..off + n]);
            self.pos += n as u64;
            Ok(n)
        }
    }

    impl Seek for RangeReader {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            let new = match to {
                SeekFrom::Start(p) => p as i128,
                SeekFrom::End(d) => self.len as i128 + d as i128,
                SeekFrom::Current(d) => self.pos as i128 + d as i128,
            };
            if new < 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "seek before start",
                ));
            }
            self.pos = new as u64;
            Ok(self.pos)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nuget_urls() {
        let [a, b] = nuget::package_urls("Microsoft.AI.DirectML", "1.15.4");
        assert_eq!(
            a,
            "https://api.nuget.org/v3-flatcontainer/microsoft.ai.directml/1.15.4/microsoft.ai.directml.1.15.4.nupkg"
        );
        assert_eq!(
            b,
            "https://www.nuget.org/api/v2/package/Microsoft.AI.DirectML/1.15.4"
        );
    }

    /// Network test: reads the two DLLs out of the real NuGet packages with range requests and
    /// checks their pinned sha256. Opt in with `STEM_TEST_NUGET=1`.
    #[test]
    fn nuget_range_extract() {
        if std::env::var_os("STEM_TEST_NUGET").is_none() {
            eprintln!("skipped: set STEM_TEST_NUGET=1 to download from nuget.org");
            return;
        }
        let dir = std::env::temp_dir().join(format!("stem-models-nuget-{}", std::process::id()));
        let mut last = None;
        nuget::fetch_windows_runtime(&dir, &mut |p| last = Some(p)).unwrap();
        assert!(dir.join("DirectML.dll").is_file());
        assert!(dir.join("onnxruntime.dll").is_file());
        assert_eq!(
            last,
            Some(Progress::Downloading {
                fraction: Some(1.0)
            })
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
