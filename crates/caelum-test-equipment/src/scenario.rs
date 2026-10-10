//! Scenario player (CR-012 §8): a test writes what an emulated device sends and when; the player
//! turns each line into the exact SIG payload. It never invents a value, never repeats the last
//! one and stops at the end of the script (the data then goes stale, as with a real device).
//!
//! Format, one reading per line (`#` starts a comment):
//! ```text
//! # seconds-after-connect  kind  values...
//! 0   plx  97 72      # continuous oximeter: SpO2 %, pulse /min
//! 1   hr   75         # heart-rate strap: bpm
//! ```

use crate::sig::{self, uuid};

/// One scripted notification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    /// Seconds after the emulated device connects.
    pub at_ms: u64,
    /// Characteristic it is sent on.
    pub characteristic: u16,
    /// The payload a real device would send.
    pub payload: Vec<u8>,
}

/// A script that could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("scenario line {line}: {reason}")]
pub struct ScenarioError {
    /// 1-based line number.
    pub line: usize,
    /// What is wrong.
    pub reason: String,
}

fn num<T: std::str::FromStr>(
    field: Option<&str>,
    line: usize,
    what: &str,
) -> Result<T, ScenarioError> {
    field
        .and_then(|f| f.parse().ok())
        .ok_or_else(|| ScenarioError {
            line,
            reason: format!("{what} missing or not a number"),
        })
}

/// Parses a scenario. Steps must be in time order.
pub fn parse(text: &str) -> Result<Vec<Step>, ScenarioError> {
    let mut steps: Vec<Step> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let body = raw.split('#').next().unwrap_or("").trim();
        if body.is_empty() {
            continue;
        }
        let mut f = body.split_whitespace();
        let at_s: f64 = num(f.next(), line, "time")?;
        if !at_s.is_finite() || at_s < 0.0 {
            return Err(ScenarioError {
                line,
                reason: "time must be zero or positive".into(),
            });
        }
        let at_ms = (at_s * 1000.0).round() as u64;
        let (characteristic, payload) = match f.next() {
            Some("plx") => {
                let spo2: i16 = num(f.next(), line, "SpO2")?;
                let pulse: i16 = num(f.next(), line, "pulse")?;
                (
                    uuid::PLX_CONTINUOUS,
                    sig::plx_continuous(sig::sfloat(spo2, 0), sig::sfloat(pulse, 0), 0),
                )
            }
            Some("hr") => {
                let bpm: u8 = num(f.next(), line, "heart rate")?;
                (
                    uuid::HEART_RATE_MEASUREMENT,
                    sig::heart_rate(bpm, Some(true), &[]),
                )
            }
            other => {
                return Err(ScenarioError {
                    line,
                    reason: format!("unknown kind {}", other.unwrap_or("(none)")),
                })
            }
        };
        if f.next().is_some() {
            return Err(ScenarioError {
                line,
                reason: "too many values".into(),
            });
        }
        if steps.last().is_some_and(|s| s.at_ms > at_ms) {
            return Err(ScenarioError {
                line,
                reason: "steps must be in time order".into(),
            });
        }
        steps.push(Step {
            at_ms,
            characteristic,
            payload,
        });
    }
    Ok(steps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cr_012_scenarios_become_exact_device_payloads() {
        let steps = parse("# test\n0 plx 97 72\n1.5 hr 75 # strap\n\n").unwrap();
        assert_eq!(steps.len(), 2);
        assert_eq!(steps[0].characteristic, uuid::PLX_CONTINUOUS);
        assert_eq!(
            steps[0].payload,
            sig::plx_continuous(sig::sfloat(97, 0), sig::sfloat(72, 0), 0)
        );
        assert_eq!(
            (steps[1].at_ms, steps[1].characteristic),
            (1500, uuid::HEART_RATE_MEASUREMENT)
        );
    }

    #[test]
    fn cr_012_bad_scenarios_are_refused_with_the_line() {
        for (text, line) in [
            ("x plx 97 72", 1),
            ("0 plx 97", 1),
            ("0 ecg 1", 1),
            ("0 hr 75 76", 1),
            ("2 hr 75\n1 hr 76", 2),
            ("-1 hr 75", 1),
            ("0 hr 300", 1),
        ] {
            assert_eq!(parse(text).map_err(|e| e.line), Err(line), "{text}");
        }
    }
}
