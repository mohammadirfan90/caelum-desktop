//! Advisory models: model packages, registry, feature builders, a time-limited runtime, ONNX
//! Runtime backend and explanations (F-ML-01..04, CR-018).
//!
//! Model output never raises, suppresses, delays or downgrades an alarm (SR-002): results and
//! notices are observe-level display types ([`advisory`]); the only path into the rule engine is
//! the observe-only gate, which admits a model-derived parameter only when every rule on it is an
//! observe-level threshold rule that touches nothing else.
//!
//! Flow: [`package::load_dir`] verifies a package (signature of kind `model`, pinned model hash)
//! → [`registry::Registry::install`] creates its runtime (UNAVAILABLE without one) →
//! [`registry::Registry::evaluate`] builds features (missing data → no output), runs the model
//! under its time limit, rejects implausible outputs and explains every output (F-ML-04) →
//! [`advisory::AdvisoryNotice::from_result`] / [`advisory::ObserveOnlyInput::apply`].
//!
//! No trained model ships with this crate; packages come from the `ml/` pipeline (docs/08 §3).

pub mod advisory;
pub mod explain;
pub mod features;
pub mod manifest;
#[cfg(feature = "onnx")]
pub mod onnx;
pub mod package;
pub mod registry;
pub mod runtime;
