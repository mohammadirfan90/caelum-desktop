//! Test equipment for caelum-ml tests: signing keys, package builder and deterministic
//! `ModelRuntime` implementations. Nothing here is a trained or clinical model: the "model file"
//! of a test package is a list of weights for a fixed linear-logistic function.

#![allow(
    dead_code,
    missing_docs,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::panic
)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use caelum_alarms::signing::{self, ConfigEnvelope, KeyClass, SignerRole, TrustedKey};
use caelum_domain::ids::CrewMemberId;
use caelum_domain::observation::Quality;
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_ml::features::Sample;
use caelum_ml::package::{self, VerifiedPackage};
use caelum_ml::runtime::{ModelRuntime, RuntimeError, RuntimeFactory};
use ed25519_dalek::SigningKey;

pub const NS: i64 = 1_000_000_000;

pub fn author_key() -> SigningKey {
    SigningKey::from_bytes(&[7u8; 32])
}

pub fn other_key() -> SigningKey {
    SigningKey::from_bytes(&[9u8; 32])
}

pub fn keys(class: KeyClass) -> Vec<TrustedKey> {
    vec![TrustedKey {
        class,
        key: author_key().verifying_key(),
    }]
}

pub fn crew() -> CrewMemberId {
    CrewMemberId::from_uuid(uuid_from(1))
}

pub fn uuid_from(n: u128) -> uuid::Uuid {
    uuid::Uuid::from_u128(n)
}

/// Model bytes: weights for the test linear-logistic runtime (one per input, then the bias).
pub fn model_bytes(weights: &[f32], bias: f32) -> Vec<u8> {
    let mut s: Vec<String> = weights.iter().map(|w| w.to_string()).collect();
    s.push(bias.to_string());
    s.join(" ").into_bytes()
}

/// A manifest for a two-feature AF-like test model (`hr_mean`, `rr_rmssd`) with one output
/// `af_probability` reported as the parameter of the same name.
pub fn manifest_text(sha256: &str, version: &str, status: &str) -> String {
    let evidence = if status == "VALIDATED" {
        "evidence = \"report frozen-set-2026-10 (sha256 0123)\"\n"
    } else {
        ""
    };
    format!(
        r#"[model]
id = "T2"
name = "Test rhythm model (test equipment)"
version = "{version}"
file = "model.onnx"
sha256 = "{sha256}"

[validation]
status = "{status}"
gate = "AF F1 >= 0.80 subject-wise (test equipment, not a real gate result)"
{evidence}
[card]
intended_use = "Test equipment only"
training_data = "None: fixed test weights"
evaluation = "None"
limitations = "Not a clinical model"
approver = "Test"

[runtime]
time_limit_ms = 200
input_name = "features"
output_name = "scores"

[[feature]]
name = "hr_mean"
parameter = "heart_rate"
aggregate = "mean"
window_s = 60
min_samples = 5
max_gap_s = 10
reference = 70.0

[[feature]]
name = "rr_rmssd"
parameter = "rr_interval"
aggregate = "rmssd"
window_s = 60
min_samples = 5
max_gap_s = 10
reference = 30.0

[[output]]
name = "af_probability"
label = "AF probability"
index = 0
min = 0.0
max = 1.0
parameter = "af_probability"
notice = {{ op = ">", threshold = 0.8, title = "Irregular rhythm suspected", action = "Record a 12-lead ECG with the onboard kit." }}

[explanation]
top_k = 2
"#
    )
}

pub fn envelope(
    kind: &str,
    bytes: &[u8],
    version: &str,
    key: &SigningKey,
    role: SignerRole,
) -> String {
    let mut entry = signing::sign_config(kind, bytes, version, key);
    entry.role = role;
    let env = ConfigEnvelope {
        kind: kind.to_owned(),
        version: version.to_owned(),
        sha256: signing::digest(bytes),
        signatures: vec![entry],
    };
    toml::to_string(&env).expect("envelope serialises")
}

pub struct Built {
    pub manifest: Vec<u8>,
    pub envelope: String,
    pub model: Vec<u8>,
}

pub fn build_package(weights: &[f32], bias: f32, version: &str, status: &str) -> Built {
    let model = model_bytes(weights, bias);
    let manifest = manifest_text(&signing::digest(&model), version, status).into_bytes();
    let envelope = envelope(
        "model",
        &manifest,
        version,
        &author_key(),
        SignerRole::Author,
    );
    Built {
        manifest,
        envelope,
        model,
    }
}

pub fn verified(weights: &[f32], bias: f32, version: &str) -> VerifiedPackage {
    let b = build_package(weights, bias, version, "NOT_VALIDATED");
    package::verify(
        &b.manifest,
        &b.envelope,
        &b.model,
        &keys(KeyClass::Development),
        signing::TrustPolicy::Development,
    )
    .expect("test package verifies")
}

/// Deterministic linear-logistic runtime: sigmoid(w·x + b) as output 0.
pub struct Linear {
    pub weights: Vec<f32>,
    pub bias: f32,
}

impl ModelRuntime for Linear {
    fn run(&self, input: &[f32]) -> Result<Vec<f32>, RuntimeError> {
        if input.len() != self.weights.len() {
            return Err(RuntimeError::Run("input length".to_owned()));
        }
        let z: f32 = self
            .weights
            .iter()
            .zip(input)
            .map(|(w, x)| w * x)
            .sum::<f32>()
            + self.bias;
        Ok(vec![1.0 / (1.0 + (-z).exp())])
    }
}

/// Factory that parses the verified model bytes as `Linear` weights.
pub struct LinearFactory;

impl RuntimeFactory for LinearFactory {
    fn load(&self, package: &VerifiedPackage) -> Result<Arc<dyn ModelRuntime>, RuntimeError> {
        let text = std::str::from_utf8(package.model_bytes())
            .map_err(|e| RuntimeError::Load(e.to_string()))?;
        let mut v: Vec<f32> = text
            .split_whitespace()
            .map(str::parse)
            .collect::<Result<_, _>>()
            .map_err(|e: std::num::ParseFloatError| RuntimeError::Load(e.to_string()))?;
        let bias = v
            .pop()
            .ok_or_else(|| RuntimeError::Load("empty".to_owned()))?;
        Ok(Arc::new(Linear { weights: v, bias }))
    }
}

/// What an adversarial runtime does.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Adversary {
    /// Always returns this value.
    Constant(f32),
    /// Returns this value for the first call, then a sane value (breaks only the explanation).
    FirstThen(f32, f32),
    /// Sleeps past any time limit.
    Hang(Duration),
    /// Panics.
    Panic,
    /// Returns an error.
    Fail,
    /// Returns no values.
    Empty,
    /// Returns many values.
    Many(f32),
}

pub struct Adversarial {
    pub mode: Adversary,
    pub calls: AtomicUsize,
}

impl ModelRuntime for Adversarial {
    fn run(&self, _input: &[f32]) -> Result<Vec<f32>, RuntimeError> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst);
        match self.mode {
            Adversary::Constant(v) => Ok(vec![v]),
            Adversary::FirstThen(a, b) => Ok(vec![if n == 0 { a } else { b }]),
            Adversary::Hang(d) => {
                std::thread::sleep(d);
                Ok(vec![0.99])
            }
            Adversary::Panic => panic!("adversarial runtime panics"),
            Adversary::Fail => Err(RuntimeError::Run("adversarial failure".to_owned())),
            Adversary::Empty => Ok(vec![]),
            Adversary::Many(v) => Ok(vec![v; 10_000]),
        }
    }
}

pub struct AdversarialFactory(pub Adversary);

impl RuntimeFactory for AdversarialFactory {
    fn load(&self, _package: &VerifiedPackage) -> Result<Arc<dyn ModelRuntime>, RuntimeError> {
        Ok(Arc::new(Adversarial {
            mode: self.0,
            calls: AtomicUsize::new(0),
        }))
    }
}

pub fn sample(parameter: Parameter, value: f64, t_s: i64, quality: Quality) -> Sample {
    Sample {
        parameter,
        value,
        at: TaiNs(t_s * NS),
        quality,
    }
}

/// HR and RR samples every 2 s over `[from_s, to_s]`.
pub fn samples(from_s: i64, to_s: i64, hr: f64) -> Vec<Sample> {
    let mut out = Vec::new();
    let mut t = from_s;
    let mut alt = false;
    while t <= to_s {
        out.push(sample(Parameter::HeartRate, hr, t, Quality::Good));
        out.push(sample(
            Parameter::RrInterval,
            if alt { 800.0 } else { 860.0 },
            t,
            Quality::Good,
        ));
        alt = !alt;
        t += 2;
    }
    out
}
