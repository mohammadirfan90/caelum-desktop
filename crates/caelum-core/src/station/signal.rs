//! Signal processing in the station (M7 integration; F-SIG-01, F-SIG-02, F-ACQ-03, F-ALM-08,
//! CR-013): ECG waveforms are stored as 1 s chunks and run through the streaming QRS detector;
//! every detected beat is a `qrs` event for `asystole.critical`, and once per second the ECG SQI
//! (the rule's gate) and heart rate are stored as derived observations and evaluated. Personal
//! baselines are estimated on the tick and given to the engine with each observation.

use caelum_alarms::engine::BaselineStats;
use caelum_devices::ghs::{Waveform, WaveformSignal, WaveformUnit};
use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::observation::{Classification, MeasurementMode, Quality};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_ingest::{Observation, Provenance, TimeSource, Value};
use caelum_signal::baseline::{
    draft_spec, estimate_baseline, ActivityContext, BaselineOutcome, BaselineSample,
};
use caelum_signal::ecg::EcgProcessor;
use caelum_store::{BaselineRecord, WaveformChunkRecord};

use super::{Station, StationError};
use crate::acquisition::observation_record;
use crate::alarms::Output;

const NS: i64 = 1_000_000_000;
/// Activity level at or below which a sample counts as at rest (CR-013 §8, ⚕ DRAFT).
pub const AT_REST_MAX: f64 = 0.3;
/// How often baselines are re-estimated, s.
pub const BASELINE_EVERY_S: i64 = 600;
/// Algorithm of the ECG quality taken from the device's own status (SR-005).
pub const DEVICE_STATUS_SQI: &str = "sqi-device-status/1";
/// A gap larger than this restarts the ECG stream (the detector relearns), s.
const STREAM_GAP_S: f64 = 0.5;

/// ECG-derived respiration window, s (F-SIG-03; the algorithm's minimum is 20 s).
pub const EDR_WINDOW_S: f64 = 30.0;

/// Beats collected for ECG-derived respiration over one window.
#[derive(Debug, Clone)]
struct EdrWindow {
    start: i64,
    beats: Vec<caelum_signal::respiration::EdrBeat>,
    worst: Quality,
}

/// One device's running ECG stream.
pub(super) struct EcgStream {
    edr: Option<EdrWindow>,
    processor: EcgProcessor,
    /// TAI of the stream's first sample.
    origin: i64,
    fs: f64,
    /// Samples pushed so far.
    pushed: u64,
}

fn quality_name(q: Quality) -> &'static str {
    match q {
        Quality::Good => "good",
        Quality::Degraded => "degraded",
        Quality::Poor => "poor",
        Quality::Unusable => "unusable",
    }
}

/// i16 little-endian microvolts (the encoding the ECG strip reads), clamped to ±32.767 mV.
fn encode_uv(samples_mv: &[f64]) -> Vec<u8> {
    samples_mv
        .iter()
        .flat_map(|mv| {
            let uv = (mv * 1000.0).round().clamp(-32_767.0, 32_767.0) as i16;
            uv.to_le_bytes()
        })
        .collect()
}

impl Station {
    /// Processes the ECG waveforms of one notification (F-ACQ-03 → F-SIG-01/02 → asystole).
    pub fn ingest_waveforms(
        &mut self,
        device: DeviceId,
        waveforms: Vec<(CrewMemberId, Waveform)>,
    ) -> Result<Output, StationError> {
        self.require_acting()?;
        let mut out = Output::default();
        let now = self.now()?;
        for (crew, w) in waveforms {
            let WaveformSignal::Ecg(_) = w.signal else {
                continue;
            };
            if w.unit != WaveformUnit::Millivolt
                || w.samples.is_empty()
                || !w.sample_rate_hz.is_finite()
                || w.sample_rate_hz <= 0.0
            {
                continue;
            }
            let fs = w.sample_rate_hz;
            let duration_ns = (w.samples.len() as f64 / fs * 1e9) as i64;
            // A waveform without a device time is placed so that it ends at receipt.
            let start = w.start.map_or(now.0 - duration_ns, |t| t.0);
            self.db.insert_waveform_chunk(
                &crew.to_string(),
                &WaveformChunkRecord {
                    device_id: Some(device.to_string()),
                    signal: "ecg".to_owned(),
                    start_tai: start,
                    sample_rate: fs,
                    encoding: "i16le".to_owned(),
                    data: encode_uv(&w.samples),
                    quality: quality_name(w.quality).to_owned(),
                },
            )?;
            // One stream per device; a gap, a rate change or an unusable chunk restarts it.
            let restart = match self.ecg.get(&device) {
                Some(s) => {
                    let expected = s.origin + (s.pushed as f64 / s.fs * 1e9) as i64;
                    s.fs != fs || ((start - expected) as f64 / 1e9).abs() > STREAM_GAP_S
                }
                None => true,
            };
            if restart {
                let Ok(processor) = EcgProcessor::new(fs) else {
                    continue;
                };
                self.ecg.insert(
                    device,
                    EcgStream {
                        edr: None,
                        processor,
                        origin: start,
                        fs,
                        pushed: 0,
                    },
                );
            }
            if w.quality == Quality::Unusable {
                // SR-005: the device says the signal is unusable (lead off). Record that as the
                // ECG quality, so the asystole gate closes now instead of keeping the last good
                // SQI; the signal technical alarm is raised by the engine.
                self.ecg.remove(&device);
                let end = TaiNs(start + duration_ns);
                let o = self.store_derived(
                    crew,
                    device,
                    Parameter::SqiEcg,
                    0.0,
                    Quality::Unusable,
                    DEVICE_STATUS_SQI,
                    end,
                )?;
                out.events.extend(o.events);
                out.notices.extend(o.notices);
                continue;
            }
            let Some(stream) = self.ecg.get_mut(&device) else {
                continue;
            };
            let result = stream.processor.push(&w.samples);
            let (origin, sfs) = (stream.origin, stream.fs);
            stream.pushed += w.samples.len() as u64;
            let Ok(result) = result else {
                // Non-finite samples are a data gap: restart on the next chunk.
                self.ecg.remove(&device);
                continue;
            };
            let at = |sample: u64| TaiNs(origin + (sample as f64 / sfs * 1e9) as i64);
            // F-SIG-03: R amplitudes for ECG-derived respiration, one window at a time.
            let worst = result
                .reports
                .iter()
                .map(|r| r.sqi.quality)
                .fold(Quality::Good, Quality::max);
            let mut edr_done = None;
            if let Some(stream) = self.ecg.get_mut(&device) {
                for q in &result.qrs {
                    let t = at(q.sample).0;
                    let w = stream.edr.get_or_insert(EdrWindow {
                        start: t,
                        beats: Vec::new(),
                        worst: Quality::Good,
                    });
                    w.beats.push(caelum_signal::respiration::EdrBeat {
                        t_s: (t - w.start) as f64 / 1e9,
                        amplitude_mv: q.amplitude_mv,
                    });
                }
                if let Some(w) = stream.edr.as_mut() {
                    w.worst = w.worst.max(worst);
                }
                let span = stream
                    .edr
                    .as_ref()
                    .and_then(|w| w.beats.last().map(|b| b.t_s))
                    .unwrap_or(0.0);
                if span >= EDR_WINDOW_S {
                    edr_done = stream.edr.take();
                }
            }
            for q in result.qrs {
                let o = self.alarms.event(&self.db, crew, "qrs", at(q.sample))?;
                out.events.extend(o.events);
                out.notices.extend(o.notices);
            }
            // One respiration source per device: a device with a RIP band is measured there.
            let has_rip = self.kit_has_rip(device);
            if let (Some(w), false) = (edr_done, has_rip) {
                let end = TaiNs(w.start + (EDR_WINDOW_S * 1e9) as i64);
                let beats: Vec<_> = w
                    .beats
                    .into_iter()
                    .filter(|b| b.t_s < EDR_WINDOW_S)
                    .collect();
                if let Ok(est) =
                    caelum_signal::respiration::respiration_from_edr(&beats, EDR_WINDOW_S, w.worst)
                {
                    if let Some(rate) = est.rate {
                        let o = self.store_derived(
                            crew,
                            device,
                            Parameter::RespirationRate,
                            rate.value,
                            rate.quality,
                            rate.algorithm,
                            end,
                        )?;
                        out.events.extend(o.events);
                        out.notices.extend(o.notices);
                    }
                }
            }
            for r in result.reports {
                let t = at(r.end_sample);
                let mut derived = vec![(
                    Parameter::SqiEcg,
                    r.sqi.score,
                    r.sqi.quality,
                    r.sqi.algorithm,
                )];
                if let Some(hr) = r.heart_rate {
                    derived.push((Parameter::HeartRate, hr.value, hr.quality, hr.algorithm));
                }
                for (parameter, value, quality, algorithm) in derived {
                    let o =
                        self.store_derived(crew, device, parameter, value, quality, algorithm, t)?;
                    out.events.extend(o.events);
                    out.notices.extend(o.notices);
                }
            }
        }
        Ok(out)
    }

    /// Stores and evaluates one derived observation (docs/03 §3 step 4).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn store_derived(
        &mut self,
        crew: CrewMemberId,
        device: DeviceId,
        parameter: Parameter,
        value: f64,
        quality: Quality,
        algorithm: &'static str,
        at: TaiNs,
    ) -> Result<Output, StationError> {
        let now = self.now()?;
        let obs = Observation {
            crew_member_id: crew,
            device_id: device,
            parameter,
            value: Value::Number(value),
            unit: parameter.spec().unit,
            measured_at: at,
            received_at: now,
            quality,
            classification: Classification::Derived,
            mode: MeasurementMode::Continuous,
            provenance: Provenance {
                time: TimeSource::Receipt,
                original: None,
                site: None,
            },
        };
        let mut row =
            observation_record(&obs).map_err(|e| StationError::Rejected(e.to_string()))?;
        row.algorithm_version = Some(algorithm.to_owned());
        self.db.insert_observation(&row)?;
        self.observe(&obs)
    }

    /// Stores and evaluates one derived state (e.g. `sleep_stage`).
    #[allow(clippy::too_many_arguments)]
    pub(super) fn store_derived_state(
        &mut self,
        crew: CrewMemberId,
        device: DeviceId,
        parameter: Parameter,
        state: &'static str,
        quality: Quality,
        algorithm: &'static str,
        at: TaiNs,
    ) -> Result<Output, StationError> {
        let now = self.now()?;
        let obs = Observation {
            crew_member_id: crew,
            device_id: device,
            parameter,
            value: Value::State(state),
            unit: parameter.spec().unit,
            measured_at: at,
            received_at: now,
            quality,
            classification: Classification::Derived,
            mode: MeasurementMode::Continuous,
            provenance: Provenance {
                time: TimeSource::Receipt,
                original: None,
                site: None,
            },
        };
        let mut row =
            observation_record(&obs).map_err(|e| StationError::Rejected(e.to_string()))?;
        row.algorithm_version = Some(algorithm.to_owned());
        self.db.insert_observation(&row)?;
        self.observe(&obs)
    }

    /// Re-estimates every active crew member's baselines (F-ALM-08, CR-012 §6), every
    /// [`BASELINE_EVERY_S`]. Unknown activity is never taken as rest (SR-018).
    pub(super) fn refresh_baselines(&mut self, now: TaiNs) -> Result<(), StationError> {
        if self
            .baselines_at
            .is_some_and(|t| now.0 - t < BASELINE_EVERY_S * NS)
        {
            return Ok(());
        }
        self.baselines_at = Some(now.0);
        let crews: Vec<String> = self
            .db
            .crew_members()?
            .into_iter()
            .filter(|c| c.active)
            .map(|c| c.id)
            .collect();
        for crew in crews {
            let Ok(crew_id) = crew.parse::<CrewMemberId>() else {
                continue;
            };
            for parameter in [
                Parameter::HeartRate,
                Parameter::HrvRmssd,
                Parameter::RespirationRate,
                Parameter::CoreTemp,
                Parameter::Spo2,
                Parameter::Weight,
            ] {
                let Some(spec) = draft_spec(parameter) else {
                    continue;
                };
                let from = now.0 - spec.window_s * NS;
                let rows =
                    self.db
                        .observations_between(&crew, parameter.spec().key, from, now.0)?;
                let activity = if spec.at_rest {
                    self.db.observations_between(
                        &crew,
                        Parameter::ActivityLevel.spec().key,
                        from - 60 * NS,
                        now.0,
                    )?
                } else {
                    Vec::new()
                };
                let samples: Vec<BaselineSample> = rows
                    .iter()
                    .filter(|r| r.classification != "simulated")
                    .filter_map(|r| {
                        let quality = match r.quality.as_str() {
                            "good" => Quality::Good,
                            "degraded" => Quality::Degraded,
                            "poor" => Quality::Poor,
                            _ => Quality::Unusable,
                        };
                        // The newest activity level within 60 s before the sample.
                        let level = activity
                            .iter()
                            .rev()
                            .find(|a| {
                                a.measured_at_tai <= r.measured_at_tai
                                    && r.measured_at_tai - a.measured_at_tai <= 60 * NS
                            })
                            .and_then(|a| a.value);
                        Some(BaselineSample {
                            at: TaiNs(r.measured_at_tai),
                            value: r.value?,
                            quality,
                            activity: ActivityContext::from_activity_level(level, AT_REST_MAX),
                        })
                    })
                    .collect();
                if let BaselineOutcome::Established(b) = estimate_baseline(&samples, spec, now) {
                    self.db.upsert_baseline(
                        &crew,
                        parameter.spec().key,
                        &BaselineRecord {
                            median: b.median,
                            mad: b.mad,
                            n: i64::try_from(b.n).unwrap_or(i64::MAX),
                            window: format!("{}s", spec.window_s),
                            status: "established".to_owned(),
                            method_version: b.method_version.to_owned(),
                        },
                    )?;
                    self.baselines.insert(
                        (crew_id, parameter),
                        BaselineStats {
                            median: b.median,
                            mad: b.mad,
                            n: u32::try_from(b.n).unwrap_or(u32::MAX),
                        },
                    );
                }
            }
        }
        Ok(())
    }

    /// The baseline the engine should use for a reading (F-ALM-08).
    pub(super) fn baseline_for(
        &self,
        crew: CrewMemberId,
        parameter: Parameter,
    ) -> Option<BaselineStats> {
        self.baselines.get(&(crew, parameter)).copied()
    }
}
