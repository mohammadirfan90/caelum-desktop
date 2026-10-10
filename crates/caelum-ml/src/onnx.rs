//! The ONNX Runtime backend (feature `onnx`, CR-018 §2; F-ML-01, docs/08 §1).
//!
//! `ort` with `load-dynamic`: the ONNX Runtime library is loaded at run time from the path in
//! configuration, after its SHA-256 matches the pinned checksum of the tool-cache binary
//! (docs/OFFLINE_BUILD.md). If the path is not configured, the file is absent, or the checksum or
//! the library's API version does not match, every model is UNAVAILABLE: there is no fallback.
//! CPU only (no execution providers are registered), fixed thread counts. The library is loaded
//! once per process; a later factory naming a different library is UNAVAILABLE.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use ort::session::Session;
use ort::value::Tensor;

use crate::package::VerifiedPackage;
use crate::runtime::{ModelRuntime, RuntimeError, RuntimeFactory};

/// Runtime configuration (from the signed application configuration).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrtConfig {
    /// Path of the ONNX Runtime library (`onnxruntime.dll`, `libonnxruntime.so`).
    pub library: PathBuf,
    /// Pinned SHA-256 of that library, lower-case hex (tool-cache `MANIFEST.sha256`).
    pub library_sha256: String,
    /// Intra-op threads per session (fixed; at least 1).
    pub intra_threads: usize,
}

/// The library loaded into this process, if any.
static LOADED: Mutex<Option<PathBuf>> = Mutex::new(None);

fn unavailable(reason: impl Into<String>) -> RuntimeError {
    RuntimeError::Unavailable(reason.into())
}

/// Checks the pinned checksum and loads the library once per process.
fn ensure_loaded(config: &OrtConfig) -> Result<(), RuntimeError> {
    let mut loaded = LOADED
        .lock()
        .map_err(|_| unavailable("runtime loader state poisoned"))?;
    if let Some(path) = loaded.as_ref() {
        return if path == &config.library {
            Ok(())
        } else {
            Err(unavailable(format!(
                "another ONNX Runtime library ({}) is already loaded",
                path.display()
            )))
        };
    }
    let path: &Path = &config.library;
    if path.as_os_str().is_empty() {
        return Err(unavailable("no ONNX Runtime library configured"));
    }
    if !path.is_file() {
        return Err(unavailable(format!(
            "ONNX Runtime library not found at {}",
            path.display()
        )));
    }
    let bytes = std::fs::read(path)
        .map_err(|e| unavailable(format!("cannot read {}: {e}", path.display())))?;
    let found = caelum_alarms::signing::digest(&bytes);
    if found != config.library_sha256 {
        return Err(unavailable(format!(
            "{} is not the pinned ONNX Runtime binary (SHA-256 {found})",
            path.display()
        )));
    }
    ort::init_from(path)
        .map_err(|e| unavailable(e.to_string()))?
        .with_name("caelum-ml")
        .commit();
    *loaded = Some(config.library.clone());
    Ok(())
}

/// Creates ONNX Runtime sessions for verified packages.
#[derive(Debug, Clone)]
pub struct OrtFactory {
    config: OrtConfig,
}

impl OrtFactory {
    /// A factory for `config`. Nothing is loaded until the first model.
    pub fn new(config: OrtConfig) -> Self {
        Self { config }
    }
}

impl RuntimeFactory for OrtFactory {
    fn load(&self, package: &VerifiedPackage) -> Result<Arc<dyn ModelRuntime>, RuntimeError> {
        ensure_loaded(&self.config)?;
        let load = |e: &dyn std::fmt::Display| RuntimeError::Load(e.to_string());
        let mut builder = Session::builder()
            .map_err(|e| load(&e))?
            .with_intra_threads(self.config.intra_threads.max(1))
            .map_err(|e| load(&e))?
            .with_inter_threads(1)
            .map_err(|e| load(&e))?;
        let session = builder
            .commit_from_memory(package.model_bytes())
            .map_err(|e| load(&e))?;
        let m = package.manifest();
        Ok(Arc::new(OrtRuntime {
            session: Mutex::new(session),
            input_name: m.runtime.input_name.clone(),
            output_name: m.runtime.output_name.clone(),
        }))
    }
}

/// One ONNX Runtime session.
struct OrtRuntime {
    session: Mutex<Session>,
    input_name: String,
    output_name: String,
}

impl ModelRuntime for OrtRuntime {
    fn run(&self, input: &[f32]) -> Result<Vec<f32>, RuntimeError> {
        let run = |e: &dyn std::fmt::Display| RuntimeError::Run(e.to_string());
        let mut session = self
            .session
            .lock()
            .map_err(|_| RuntimeError::Run("session state poisoned".to_owned()))?;
        let tensor =
            Tensor::from_array(([1usize, input.len()], input.to_vec())).map_err(|e| run(&e))?;
        let outputs = session
            .run(ort::inputs![self.input_name.as_str() => tensor])
            .map_err(|e| run(&e))?;
        let value = outputs
            .get(&self.output_name)
            .ok_or_else(|| RuntimeError::Run(format!("no output `{}`", self.output_name)))?;
        let (_shape, data) = value.try_extract_tensor::<f32>().map_err(|e| run(&e))?;
        Ok(data.to_vec())
    }
}
