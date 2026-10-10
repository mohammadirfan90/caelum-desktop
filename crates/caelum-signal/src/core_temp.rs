//! Core-temperature estimate (F-SIG-05, → `core_temp`): an extended Kalman filter with heart
//! rate as an observation (Buller MJ et al., "Estimation of human core temperature from
//! sequential heart rate observations", Physiol Meas 2013;34:781–98) and, optionally, a
//! heat-flux observation `T_core ≈ T_skin + R_tissue · q` (the heat-flux, "zero-heat-flux"
//! family of methods) with a per-person calibrated tissue resistance.
//!
//! Algorithm `core-temp-ekf-hr-hf/1`. One step per minute of data (heart rate as a 1-minute
//! mean, skin temperature and heat flux as 1-minute means).
//!
//! ⚕ DRAFT (CR-013 §9), pending the provisional medical lead:
//! - The published HR model coefficients ([`BULLER_2013`]) come from terrestrial, mostly young
//!   male, exercising cohorts; their validity in microgravity (fluid shift, altered HR response)
//!   is unknown and must be validated against a reference (e.g. ingestible pill) on the ground
//!   analogue before the estimate can drive more than observation.
//! - The published filter starts from 37.1 °C. CAELUM never assumes a start value (CLAUDE.md
//!   rule 2): the filter is started from a **measured** core-equivalent temperature
//!   (`body_temp_measured` at a core-equivalent site, F-ACQ-02), supplied by the caller.
//! - The heat-flux tissue resistance has no population default: it is calibrated per person
//!   against a measured core temperature, or the heat-flux observation is not used.

use crate::{Derived, Quality, SignalError};

/// Algorithm identifier.
pub const CORE_TEMP_ALGORITHM: &str = "core-temp-ekf-hr-hf/1";

/// HR observation model `HR = c2·CT² + c1·CT + c0` and noise.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HrModel {
    /// Constant term, /min.
    pub c0: f64,
    /// Linear term, /min/°C.
    pub c1: f64,
    /// Quadratic term, /min/°C².
    pub c2: f64,
    /// Observation variance, (/min)².
    pub variance: f64,
    /// Process variance per minute, °C².
    pub process_variance_per_min: f64,
}

/// ⚕ DRAFT: Buller et al. 2013 coefficients (published values: c0 −7887.1, c1 384.4286,
/// c2 −4.5714, σ = 18.88 /min, γ = 0.022 °C per minute).
pub const BULLER_2013: HrModel = HrModel {
    c0: -7_887.1,
    c1: 384.428_6,
    c2: -4.571_4,
    variance: 18.88 * 18.88,
    process_variance_per_min: 0.022 * 0.022,
};

/// Heat-flux observation `T_core = T_skin + resistance · q`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeatFluxModel {
    /// Calibrated tissue thermal resistance, °C·m²/W (per person; no default).
    pub resistance: f64,
    /// Observation variance, °C².
    pub variance: f64,
}

/// One minute of inputs.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct CoreTempInput {
    /// Mean heart rate over the minute, /min (usable quality only).
    pub heart_rate: Option<f64>,
    /// Mean skin temperature under the heat-flux sensor, °C.
    pub skin_temp: Option<f64>,
    /// Mean heat flux out of the skin, W/m².
    pub heat_flux: Option<f64>,
}

/// The filter state.
#[derive(Debug, Clone, PartialEq)]
pub struct CoreTempFilter {
    hr: HrModel,
    hf: Option<HeatFluxModel>,
    ct: f64,
    var: f64,
}

/// ⚕ DRAFT: estimate standard deviation for GOOD (≤ 0.3 °C) and DEGRADED (≤ 0.5 °C); above
/// that the estimate is POOR.
const SD_GOOD: f64 = 0.3;
const SD_DEGRADED: f64 = 0.5;

impl CoreTempFilter {
    /// Starts the filter from a measured core-equivalent temperature (°C) and its variance
    /// (°C², e.g. the thermometer's accuracy squared).
    pub fn start(
        hr: HrModel,
        hf: Option<HeatFluxModel>,
        measured_core_c: f64,
        measured_variance: f64,
    ) -> Result<Self, SignalError> {
        let ok = [
            hr.c0,
            hr.c1,
            hr.c2,
            hr.variance,
            hr.process_variance_per_min,
            measured_core_c,
            measured_variance,
        ]
        .iter()
        .all(|v| v.is_finite());
        if !ok || hr.variance <= 0.0 || hr.process_variance_per_min < 0.0 {
            return Err(SignalError::Invalid("core-temperature model"));
        }
        if !(30.0..=44.0).contains(&measured_core_c) || measured_variance < 0.0 {
            return Err(SignalError::Invalid("core-temperature start value"));
        }
        if let Some(h) = hf {
            if !(h.resistance.is_finite() && h.resistance > 0.0)
                || !(h.variance.is_finite() && h.variance > 0.0)
            {
                return Err(SignalError::Invalid("heat-flux model"));
            }
        }
        Ok(Self {
            hr,
            hf,
            ct: measured_core_c,
            var: measured_variance,
        })
    }

    /// Advances `minutes` (> 0) and applies the observations that are present. Returns the
    /// estimate; with no observation for a while its variance grows and its quality falls.
    pub fn step(
        &mut self,
        minutes: f64,
        input: CoreTempInput,
    ) -> Result<Derived<f64>, SignalError> {
        if !(minutes.is_finite() && minutes > 0.0) {
            return Err(SignalError::Invalid("time step"));
        }
        self.var += self.hr.process_variance_per_min * minutes;
        if let Some(hr) = input.heart_rate {
            if !(hr.is_finite() && (20.0..=250.0).contains(&hr)) {
                return Err(SignalError::Invalid("heart rate"));
            }
            let m = 2.0 * self.hr.c2 * self.ct + self.hr.c1;
            let predicted = self.hr.c2 * self.ct * self.ct + self.hr.c1 * self.ct + self.hr.c0;
            let k = self.var * m / (m * m * self.var + self.hr.variance);
            self.ct += k * (hr - predicted);
            self.var *= 1.0 - k * m;
        }
        if let (Some(model), Some(skin), Some(q)) = (self.hf, input.skin_temp, input.heat_flux) {
            if !(skin.is_finite() && q.is_finite()) {
                return Err(SignalError::NonFinite);
            }
            let obs = skin + model.resistance * q;
            let k = self.var / (self.var + model.variance);
            self.ct += k * (obs - self.ct);
            self.var *= 1.0 - k;
        }
        Ok(self.estimate())
    }

    /// The current estimate.
    pub fn estimate(&self) -> Derived<f64> {
        let sd = self.var.max(0.0).sqrt();
        let quality = if sd <= SD_GOOD {
            Quality::Good
        } else if sd <= SD_DEGRADED {
            Quality::Degraded
        } else {
            Quality::Poor
        };
        Derived {
            value: self.ct,
            quality,
            algorithm: CORE_TEMP_ALGORITHM,
        }
    }

    /// Estimate variance, °C².
    pub fn variance(&self) -> f64 {
        self.var
    }
}

/// Calibrates the heat-flux tissue resistance from paired measurements (measured core °C, skin
/// °C, heat flux W/m²) by least squares through the origin of `core − skin = R · q`. Needs at
/// least 3 pairs with |q| ≥ 5 W/m² and a positive result.
pub fn calibrate_tissue_resistance(pairs: &[(f64, f64, f64)]) -> Result<f64, SignalError> {
    let usable: Vec<&(f64, f64, f64)> = pairs
        .iter()
        .filter(|(c, s, q)| c.is_finite() && s.is_finite() && q.is_finite() && q.abs() >= 5.0)
        .collect();
    if usable.len() < 3 {
        return Err(SignalError::Invalid("too few heat-flux calibration pairs"));
    }
    let num: f64 = usable.iter().map(|(c, s, q)| (c - s) * q).sum();
    let den: f64 = usable.iter().map(|(_, _, q)| q * q).sum();
    let r = num / den;
    if r.is_finite() && r > 0.0 {
        Ok(r)
    } else {
        Err(SignalError::Invalid("heat-flux calibration result"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hr_for(ct: f64) -> f64 {
        BULLER_2013.c2 * ct * ct + BULLER_2013.c1 * ct + BULLER_2013.c0
    }

    /// F-SIG-05: sustained heart rate consistent with 38.5 °C pulls the estimate up; the
    /// estimate stays in range and keeps its algorithm version.
    #[test]
    fn f_sig_05_hr_drives_the_estimate() {
        let mut f = CoreTempFilter::start(BULLER_2013, None, 37.0, 0.01).unwrap();
        let target_hr = hr_for(38.5);
        let mut last = f.estimate();
        for _ in 0..120 {
            last = f
                .step(
                    1.0,
                    CoreTempInput {
                        heart_rate: Some(target_hr),
                        ..Default::default()
                    },
                )
                .unwrap();
        }
        assert!(last.value > 38.0 && last.value < 38.8, "{}", last.value);
        assert_eq!(last.algorithm, CORE_TEMP_ALGORITHM);
        assert_eq!(last.quality, Quality::Good);
    }

    /// F-SIG-05: with no observations the variance grows and the quality falls; no value is
    /// invented.
    #[test]
    fn f_sig_05_quality_falls_without_observations() {
        let mut f = CoreTempFilter::start(BULLER_2013, None, 37.2, 0.0).unwrap();
        let e = f.step(600.0, CoreTempInput::default()).unwrap();
        assert_eq!(e.value, 37.2);
        assert_eq!(e.quality, Quality::Poor);
    }

    /// F-SIG-05: the heat-flux observation, with a calibrated resistance, tracks core.
    #[test]
    fn f_sig_05_heat_flux_observation() {
        let pairs = [(37.0, 34.0, 60.0), (37.2, 34.4, 56.0), (37.5, 34.5, 60.0)];
        let r = calibrate_tissue_resistance(&pairs).unwrap();
        assert!((r - 0.05).abs() < 0.002, "{r}");
        let hf = HeatFluxModel {
            resistance: r,
            variance: 0.04,
        };
        let mut f = CoreTempFilter::start(BULLER_2013, Some(hf), 37.0, 0.01).unwrap();
        let mut e = f.estimate();
        for _ in 0..60 {
            e = f
                .step(
                    1.0,
                    CoreTempInput {
                        heart_rate: None,
                        skin_temp: Some(35.0),
                        heat_flux: Some(60.0),
                    },
                )
                .unwrap();
        }
        assert!((e.value - (35.0 + r * 60.0)).abs() < 0.1, "{}", e.value);
        assert!(calibrate_tissue_resistance(&pairs[..2]).is_err());
    }

    /// Rule 2: no start without a measured value; implausible inputs are refused.
    #[test]
    fn f_sig_05_refuses_invalid_inputs() {
        assert!(CoreTempFilter::start(BULLER_2013, None, f64::NAN, 0.01).is_err());
        assert!(CoreTempFilter::start(BULLER_2013, None, 25.0, 0.01).is_err());
        let mut f = CoreTempFilter::start(BULLER_2013, None, 37.0, 0.01).unwrap();
        assert!(f.step(0.0, CoreTempInput::default()).is_err());
        assert!(f
            .step(
                1.0,
                CoreTempInput {
                    heart_rate: Some(400.0),
                    ..Default::default()
                }
            )
            .is_err());
    }
}
