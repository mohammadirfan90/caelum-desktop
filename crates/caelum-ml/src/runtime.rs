//! Inference behind a runtime trait with a hard time limit (F-ML-01, CR-018 §4).
//!
//! [`ModelRuntime`] runs one inference; [`RuntimeFactory`] creates a runtime from a
//! [`VerifiedPackage`] only. [`run_limited`] runs an inference on a worker thread and gives up when
//! the package's time limit expires: the caller gets [`RuntimeError::TimedOut`] and no output. At
//! most one inference per model is in flight: until a timed-out worker returns, further requests
//! get [`RuntimeError::Busy`], so slow inferences never pile up. A worker that panics yields
//! [`RuntimeError::WorkerLost`]. None of these produce a value: there is no fallback guess.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use crate::package::VerifiedPackage;

/// Why an inference produced no output.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RuntimeError {
    /// The runtime library is not available (not configured, absent or not the pinned binary).
    #[error("model runtime unavailable: {0}")]
    Unavailable(String),
    /// The model could not be loaded into the runtime.
    #[error("model could not be loaded: {0}")]
    Load(String),
    /// The inference failed.
    #[error("inference failed: {0}")]
    Run(String),
    /// The inference exceeded its time limit.
    #[error("inference exceeded its {limit_ms} ms time limit")]
    TimedOut {
        /// Limit, ms.
        limit_ms: u32,
    },
    /// An earlier inference of this model is still running.
    #[error("an earlier inference of this model is still running")]
    Busy,
    /// The worker ended without a result (it panicked).
    #[error("inference worker ended without a result")]
    WorkerLost,
}

/// One loaded model. Implementations must be deterministic for a given input and must not
/// write to any device or system (CLAUDE.md rule 5): they compute an output vector only.
pub trait ModelRuntime: Send + Sync {
    /// Runs one inference on an input vector of the manifest's input length and returns the
    /// flattened output tensor.
    fn run(&self, input: &[f32]) -> Result<Vec<f32>, RuntimeError>;
}

/// Creates runtimes for verified packages.
pub trait RuntimeFactory {
    /// Loads a verified package. Only verified packages can be passed (F-ML-01).
    fn load(&self, package: &VerifiedPackage) -> Result<Arc<dyn ModelRuntime>, RuntimeError>;
}

/// A factory for builds without a model runtime: every model is UNAVAILABLE (no fallback).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoRuntime;

impl RuntimeFactory for NoRuntime {
    fn load(&self, _package: &VerifiedPackage) -> Result<Arc<dyn ModelRuntime>, RuntimeError> {
        Err(RuntimeError::Unavailable(
            "this build has no model runtime (feature `onnx` off)".to_owned(),
        ))
    }
}

/// Clears the in-flight flag when the worker finishes, also when it unwinds.
struct InFlight(Arc<AtomicBool>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Runs one inference with a hard time limit. `in_flight` is the model's in-flight flag.
pub fn run_limited(
    runtime: &Arc<dyn ModelRuntime>,
    input: Vec<f32>,
    limit_ms: u32,
    in_flight: &Arc<AtomicBool>,
) -> Result<Vec<f32>, RuntimeError> {
    if in_flight
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Err(RuntimeError::Busy);
    }
    let guard = InFlight(Arc::clone(in_flight));
    let (tx, rx) = mpsc::sync_channel(1);
    let worker_runtime = Arc::clone(runtime);
    let spawned = std::thread::Builder::new()
        .name("caelum-ml-inference".to_owned())
        .spawn(move || {
            let result = worker_runtime.run(&input);
            // Clear the flag before replying, so a caller that has the result can run again.
            drop(guard);
            // The receiver may have given up (time limit); the result is then discarded.
            let _ = tx.send(result);
        });
    if let Err(e) = spawned {
        // The closure (and with it the guard) was dropped, clearing the flag.
        return Err(RuntimeError::Run(format!(
            "cannot start inference worker: {e}"
        )));
    }
    match rx.recv_timeout(Duration::from_millis(u64::from(limit_ms))) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(RuntimeError::TimedOut { limit_ms }),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(RuntimeError::WorkerLost),
    }
}
