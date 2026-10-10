//! Sensor kit and the remaining signal algorithms in the station (M7 integration; F-ACQ-05,
//! F-SIG-03, F-SIG-04, F-SIG-05, F-SIG-07; CR-013, CR-021).
//!
//! Sensor-kit frames (ICD-SENSOR-KIT) arrive through acquisition: their catalog readings are
//! stored like any reading, the ECG goes through the existing ECG path (QRS, SQI, heart rate,
//! asystole), and the other waveforms are buffered per device. A buffer only ever holds one
//! contiguous stretch: a gap, a rate change or an unusable frame restarts it, so no window spans
//! missing data. When a buffer holds a full window, the window is processed and its outputs are
//! stored as derived observations with their algorithm versions and evaluated:
//! - RIP (30 s): respiration rate and its quality (`sqi_resp`); a pause gives no rate.
//! - PPG red + infrared (8 s): pulse rate and `sqi_ppg`. SpO₂ needs the device's own calibration
//!   curve, which ICD-SENSOR-KIT v1 does not carry (OI-3), so no SpO₂ is produced from kit PPG.
//! - Accelerometer (60 s epochs): `activity_level`, and sleep/wake (`sleep_stage` `wake` or
//!   unstaged `sleep`) once ten epochs of history exist.
//!
//! Core temperature (F-SIG-05) starts only from a measured core-equivalent temperature and is
//! then stepped each minute with the heart rate of that minute; without heart rate for
//! [`CORE_TEMP_HR_TIMEOUT_S`] it stops until the next measurement. Nothing is assumed.

use std::collections::{HashMap, VecDeque};

use caelum_devices::ghs::{Waveform, WaveformSignal};
use caelum_devices::sensor_kit::{decode_info, DeviceInfo};
use caelum_domain::ids::{CrewMemberId, DeviceId};
use caelum_domain::observation::{Quality, SiteClass};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::TaiNs;
use caelum_signal::activity::{self, Epoch, HrBaseline, SleepWake};
use caelum_signal::core_temp::{CoreTempFilter, CoreTempInput, BULLER_2013};
use caelum_signal::{ppg, respiration};

use super::runtime::Ingested;
use super::{Station, StationError};
use crate::alarms::Output;

const NS: i64 = 1_000_000_000;
/// RIP window for respiration, s (≥ the algorithm's 20 s minimum).
pub const RIP_WINDOW_S: f64 = 30.0;
/// PPG window for pulse rate, s (≥ the algorithm's 4 s minimum).
pub const PPG_WINDOW_S: f64 = 8.0;
/// Activity and sleep/wake epoch, s.
pub const EPOCH_S: f64 = 60.0;
/// Accelerometer history used as the motion context of RIP and PPG windows, s.
const MOTION_CONTEXT_S: f64 = 10.0;
/// A gap larger than this restarts a buffer, s.
const GAP_S: f64 = 0.5;
/// Epochs kept for sleep/wake (the current one and the ten before it).
const EPOCH_HISTORY: usize = 11;
/// Core-temperature filter step, s.
const CORE_TEMP_STEP_S: i64 = 60;
/// Without heart rate for this long the core-temperature estimate stops, s (⚕ DRAFT, CR-021).
pub const CORE_TEMP_HR_TIMEOUT_S: i64 = 600;
/// Variance of the measured core-equivalent start temperature, °C² (⚕ DRAFT, CR-021): the
/// ±0.3 °C maximum permissible laboratory error of ISO 80601-2-56 taken as one standard deviation.
pub const MEASURED_CORE_VARIANCE: f64 = 0.09;

/// One contiguous stretch of one signal.
#[derive(Debug, Clone)]
struct Buf<T> {
    start: i64,
    fs: f64,
    samples: Vec<T>,
}

impl<T: Copy> Buf<T> {
    fn end(&self) -> i64 {
        self.start + (self.samples.len() as f64 / self.fs * 1e9) as i64
    }

    fn secs(&self) -> f64 {
        self.samples.len() as f64 / self.fs
    }

    /// Appends, or returns false when the new samples do not continue this stretch.
    fn extend(&mut self, start: i64, fs: f64, samples: &[T]) -> bool {
        if (fs - self.fs).abs() > f64::EPSILON || ((start - self.end()) as f64 / 1e9).abs() > GAP_S
        {
            return false;
        }
        self.samples.extend_from_slice(samples);
        true
    }

    /// Takes the first `secs` of samples, if present; the rest stays.
    fn take(&mut self, secs: f64) -> Option<(i64, Vec<T>)> {
        let n = (secs * self.fs).round() as usize;
        if n == 0 || self.samples.len() < n {
            return None;
        }
        let window: Vec<T> = self.samples.drain(..n).collect();
        let start = self.start;
        self.start += (n as f64 / self.fs * 1e9) as i64;
        Some((start, window))
    }
}

fn feed<T: Copy>(slot: &mut Option<Buf<T>>, start: i64, fs: f64, samples: &[T]) {
    if let Some(b) = slot.as_mut() {
        if b.extend(start, fs, samples) {
            return;
        }
    }
    *slot = Some(Buf {
        start,
        fs,
        samples: samples.to_vec(),
    });
}

/// Running buffers of one sensor-kit device.
#[derive(Debug, Default)]
pub(super) struct KitDevice {
    rip: Option<Buf<f64>>,
    red: Option<Buf<f64>>,
    ir: Option<Buf<f64>>,
    accel: Option<Buf<[f64; 3]>>,
    /// Recent accelerometer samples for the motion context.
    recent: VecDeque<[f64; 3]>,
    recent_fs: f64,
    epochs: VecDeque<Epoch>,
    info: Option<DeviceInfo>,
}

impl KitDevice {
    fn motion(&self) -> Option<f64> {
        if self.recent.len() < 2 || self.recent_fs <= 0.0 {
            return None;
        }
        let v: Vec<[f64; 3]> = self.recent.iter().copied().collect();
        ppg::motion_from_accel(&v, self.recent_fs).ok()
    }
}

/// A running core-temperature estimate for one crew member.
#[derive(Debug, Clone)]
pub(super) struct CoreTrack {
    filter: CoreTempFilter,
    device: DeviceId,
    last_step: i64,
    last_hr: i64,
}

/// What one window produced, to store.
type Derivations = Vec<(Parameter, f64, Quality, &'static str, TaiNs)>;

impl Station {
    /// `sensor-kit` Data frame from a connected device (F-ACQ-05): decode, ingest the readings,
    /// run the waveforms through signal processing, and evaluate everything.
    pub fn ingest_sensor_kit(
        &mut self,
        device: DeviceId,
        frame: &[u8],
    ) -> Result<Ingested, StationError> {
        let now = self.now()?;
        let outcome = self
            .acquisition
            .sensor_kit(&self.db, device, frame, now)
            .map_err(|e| StationError::Rejected(e.to_string()))?;
        let mut ingested = Ingested {
            stored: outcome.observations.len(),
            ..Ingested::default()
        };
        for obs in &outcome.observations {
            ingested.crew = Some(obs.crew_member_id.to_string());
            let out = self.observe(obs)?;
            ingested.output.events.extend(out.events);
            ingested.output.notices.extend(out.notices);
        }
        if let Some((crew, _)) = outcome.waveforms.first() {
            ingested.crew = Some(crew.to_string());
        }
        let (ecg, other): (Vec<_>, Vec<_>) = outcome
            .waveforms
            .into_iter()
            .partition(|(_, w)| matches!(w.signal, WaveformSignal::Ecg(_)));
        let out = self.ingest_waveforms(device, ecg)?;
        ingested.output.events.extend(out.events);
        ingested.output.notices.extend(out.notices);
        let out = self.kit_waveforms(device, other, now)?;
        ingested.output.events.extend(out.events);
        ingested.output.notices.extend(out.notices);
        Ok(ingested)
    }

    /// The sensor kit's Info characteristic (identity and calibration records, kept for
    /// provenance; the device has already applied its calibration, ICD-SENSOR-KIT §6).
    pub fn sensor_kit_info(&mut self, device: DeviceId, bytes: &[u8]) -> Result<(), StationError> {
        let info = decode_info(bytes).map_err(|e| StationError::Rejected(e.to_string()))?;
        self.kit.entry(device).or_default().info = Some(info);
        Ok(())
    }

    /// Whether this device sends RIP (then respiration comes from RIP, not from the ECG).
    pub(super) fn kit_has_rip(&self, device: DeviceId) -> bool {
        self.kit.get(&device).is_some_and(|k| k.rip.is_some())
    }

    /// The decoded Info of a sensor-kit device, if it was read.
    pub fn sensor_kit_identity(&self, device: DeviceId) -> Option<&DeviceInfo> {
        self.kit.get(&device).and_then(|k| k.info.as_ref())
    }

    fn kit_waveforms(
        &mut self,
        device: DeviceId,
        waveforms: Vec<(CrewMemberId, Waveform)>,
        now: TaiNs,
    ) -> Result<Output, StationError> {
        let mut out = Output::default();
        let mut todo: Vec<(CrewMemberId, Derivations, Option<(i64, &'static str)>)> = Vec::new();
        let mut accel: HashMap<CrewMemberId, (i64, f64, [Vec<f64>; 3], Quality)> = HashMap::new();
        for (crew, w) in waveforms {
            if w.samples.is_empty() || !w.sample_rate_hz.is_finite() || w.sample_rate_hz <= 0.0 {
                continue;
            }
            let fs = w.sample_rate_hz;
            let duration = (w.samples.len() as f64 / fs * 1e9) as i64;
            // A frame without device time is placed so that it ends at receipt.
            let start = w.start.map_or(now.0 - duration, |t| t.0);
            let kit = self.kit.entry(device).or_default();
            match w.signal {
                WaveformSignal::RipThoracic | WaveformSignal::RipAbdominal => {
                    // One respiration source per device: thoracic when present.
                    if w.signal == WaveformSignal::RipAbdominal
                        && kit.rip.as_ref().is_some_and(|b| b.end() >= start)
                    {
                        continue;
                    }
                    if w.quality == Quality::Unusable {
                        kit.rip = None;
                        todo.push((
                            crew,
                            vec![(
                                Parameter::SqiResp,
                                0.0,
                                Quality::Unusable,
                                super::signal::DEVICE_STATUS_SQI,
                                TaiNs(start + duration),
                            )],
                            None,
                        ));
                        continue;
                    }
                    feed(&mut kit.rip, start, fs, &w.samples);
                    let motion = kit.motion();
                    while let Some((t0, window)) =
                        kit.rip.as_mut().and_then(|b| b.take(RIP_WINDOW_S))
                    {
                        let end = TaiNs(t0 + (RIP_WINDOW_S * 1e9) as i64);
                        let Ok(est) = respiration::respiration_from_rip(&window, fs, motion) else {
                            continue;
                        };
                        let mut d: Derivations = Vec::new();
                        if est.sqi.score.is_finite() {
                            d.push((
                                Parameter::SqiResp,
                                est.sqi.score,
                                est.sqi.quality,
                                est.sqi.algorithm,
                                end,
                            ));
                        }
                        if let Some(rate) = est.rate {
                            d.push((
                                Parameter::RespirationRate,
                                rate.value,
                                rate.quality,
                                rate.algorithm,
                                end,
                            ));
                        }
                        todo.push((crew, d, None));
                    }
                }
                WaveformSignal::PpgRed | WaveformSignal::PpgInfrared => {
                    let slot = if w.signal == WaveformSignal::PpgRed {
                        &mut kit.red
                    } else {
                        &mut kit.ir
                    };
                    if w.quality == Quality::Unusable {
                        kit.red = None;
                        kit.ir = None;
                        todo.push((
                            crew,
                            vec![(
                                Parameter::SqiPpg,
                                0.0,
                                Quality::Unusable,
                                super::signal::DEVICE_STATUS_SQI,
                                TaiNs(start + duration),
                            )],
                            None,
                        ));
                        continue;
                    }
                    feed(slot, start, fs, &w.samples);
                    // Process when both channels hold a window over the same stretch.
                    let aligned = match (&kit.red, &kit.ir) {
                        (Some(r), Some(i)) => r.start == i.start && r.fs == i.fs,
                        _ => false,
                    };
                    if !aligned {
                        continue;
                    }
                    let motion = kit.motion();
                    loop {
                        let (Some(r), Some(i)) = (kit.red.as_mut(), kit.ir.as_mut()) else {
                            break;
                        };
                        if r.secs() < PPG_WINDOW_S || i.secs() < PPG_WINDOW_S {
                            break;
                        }
                        let (Some((t0, red)), Some((_, ir))) =
                            (r.take(PPG_WINDOW_S), i.take(PPG_WINDOW_S))
                        else {
                            break;
                        };
                        let end = TaiNs(t0 + (PPG_WINDOW_S * 1e9) as i64);
                        // No SpO₂ calibration in ICD-SENSOR-KIT v1 (OI-3): none is passed, so
                        // no SpO₂ comes out; pulse rate and quality do.
                        let Ok(est) = ppg::spo2_and_pulse_rate(&red, &ir, fs, None, motion) else {
                            continue;
                        };
                        let mut d: Derivations = Vec::new();
                        if est.sqi.score.is_finite() {
                            d.push((
                                Parameter::SqiPpg,
                                est.sqi.score,
                                est.sqi.quality,
                                est.sqi.algorithm,
                                end,
                            ));
                        }
                        if let Some(pr) = est.pulse_rate {
                            d.push((
                                Parameter::PulseRate,
                                pr.value,
                                pr.quality,
                                pr.algorithm,
                                end,
                            ));
                        }
                        todo.push((crew, d, None));
                    }
                }
                WaveformSignal::AccelX | WaveformSignal::AccelY | WaveformSignal::AccelZ => {
                    let e = accel.entry(crew).or_insert((
                        start,
                        fs,
                        [Vec::new(), Vec::new(), Vec::new()],
                        w.quality,
                    ));
                    let axis = match w.signal {
                        WaveformSignal::AccelX => 0,
                        WaveformSignal::AccelY => 1,
                        _ => 2,
                    };
                    e.2[axis] = w.samples.clone();
                    e.3 = e.3.max(w.quality);
                }
                _ => {}
            }
        }
        // Accelerometer: the three axes of one frame become samples; epochs give activity and
        // sleep/wake.
        for (crew, (start, fs, [x, y, z], quality)) in accel {
            if x.len() != y.len() || y.len() != z.len() || x.is_empty() {
                continue;
            }
            let triples: Vec<[f64; 3]> = (0..x.len()).map(|i| [x[i], y[i], z[i]]).collect();
            let kit = self.kit.entry(device).or_default();
            if quality == Quality::Unusable {
                kit.accel = None;
                kit.recent.clear();
                continue;
            }
            kit.recent_fs = fs;
            kit.recent.extend(triples.iter().copied());
            let cap = (MOTION_CONTEXT_S * fs) as usize;
            while kit.recent.len() > cap {
                kit.recent.pop_front();
            }
            let before = kit.accel.as_ref().map(Buf::end);
            feed(&mut kit.accel, start, fs, &triples);
            if before.is_some_and(|e| kit.accel.as_ref().is_some_and(|b| b.start > e)) {
                // A gap: the sleep/wake history no longer runs epoch after epoch.
                kit.epochs.clear();
            }
            while let Some((t0, window)) = kit.accel.as_mut().and_then(|b| b.take(EPOCH_S)) {
                let end = TaiNs(t0 + (EPOCH_S * 1e9) as i64);
                let (Ok(level), Ok(motion)) = (
                    activity::activity_level(&window, fs),
                    activity::motion_g(&window, fs),
                ) else {
                    continue;
                };
                todo.push((
                    crew,
                    vec![(
                        Parameter::ActivityLevel,
                        level.value,
                        level.quality,
                        level.algorithm,
                        end,
                    )],
                    None,
                ));
                todo.push((crew, Vec::new(), Some((t0, "epoch"))));
                kit.epochs.push_back(Epoch {
                    motion_g: Some(motion),
                    heart_rate: None,
                });
                while kit.epochs.len() > EPOCH_HISTORY {
                    kit.epochs.pop_front();
                }
            }
        }
        for (crew, derived, epoch) in todo {
            for (parameter, value, quality, algorithm, at) in derived {
                let o =
                    self.store_derived(crew, device, parameter, value, quality, algorithm, at)?;
                out.events.extend(o.events);
                out.notices.extend(o.notices);
            }
            if let Some((t0, _)) = epoch {
                let o = self.sleep_epoch(crew, device, t0)?;
                out.events.extend(o.events);
                out.notices.extend(o.notices);
            }
        }
        Ok(out)
    }

    /// Completes the newest epoch with its heart rate and stores sleep/wake for it, if decided.
    fn sleep_epoch(
        &mut self,
        crew: CrewMemberId,
        device: DeviceId,
        t0: i64,
    ) -> Result<Output, StationError> {
        let end = t0 + (EPOCH_S * 1e9) as i64;
        let hr = mean_of(&self.db.observations_between(
            &crew.to_string(),
            Parameter::HeartRate.spec().key,
            t0,
            end,
        )?);
        let hr_rest = self
            .baseline_for(crew, Parameter::HeartRate)
            .map(|b| HrBaseline {
                median: b.median,
                mad: b.mad,
            });
        let kit = self.kit.entry(device).or_default();
        if let Some(last) = kit.epochs.back_mut() {
            last.heart_rate = hr;
        }
        let epochs: Vec<Epoch> = kit.epochs.iter().copied().collect();
        let Some(Some(decided)) = activity::sleep_wake(&epochs, hr_rest).last().copied() else {
            return Ok(Output::default());
        };
        let state = match decided.value {
            SleepWake::Wake => "wake",
            SleepWake::Sleep => "sleep",
        };
        self.store_derived_state(
            crew,
            device,
            Parameter::SleepStage,
            state,
            decided.quality,
            decided.algorithm,
            TaiNs(end),
        )
    }

    /// Starts (or restarts) the core-temperature estimate from a measured core-equivalent
    /// temperature (F-SIG-05). Called for every stored `body_temp_measured`.
    pub(super) fn core_temp_measured(&mut self, obs: &caelum_ingest::Observation) {
        if obs.parameter != Parameter::BodyTempMeasured
            || matches!(obs.quality, Quality::Poor | Quality::Unusable)
            || !obs
                .provenance
                .site
                .is_some_and(|s| s.class() == SiteClass::CoreEquivalent)
        {
            return;
        }
        let caelum_ingest::Value::Number(c) = obs.value else {
            return;
        };
        if let Ok(filter) = CoreTempFilter::start(BULLER_2013, None, c, MEASURED_CORE_VARIANCE) {
            self.core_temp.insert(
                obs.crew_member_id,
                CoreTrack {
                    filter,
                    device: obs.device_id,
                    last_step: obs.measured_at.0,
                    last_hr: obs.measured_at.0,
                },
            );
        }
    }

    /// Steps every running core-temperature estimate once per minute with that minute's heart
    /// rate (on the tick).
    pub(super) fn core_temp_tick(&mut self, now: TaiNs) -> Result<Output, StationError> {
        let mut out = Output::default();
        let crews: Vec<CrewMemberId> = self.core_temp.keys().copied().collect();
        for crew in crews {
            let Some(track) = self.core_temp.get(&crew).cloned() else {
                continue;
            };
            if now.0 - track.last_step < CORE_TEMP_STEP_S * NS {
                continue;
            }
            let hr = mean_of(&self.db.observations_between(
                &crew.to_string(),
                Parameter::HeartRate.spec().key,
                track.last_step + 1,
                now.0,
            )?);
            let last_hr = if hr.is_some() { now.0 } else { track.last_hr };
            if now.0 - last_hr > CORE_TEMP_HR_TIMEOUT_S * NS {
                self.core_temp.remove(&crew);
                continue;
            }
            let minutes = (now.0 - track.last_step) as f64 / (60.0 * 1e9);
            let mut filter = track.filter.clone();
            let Ok(est) = filter.step(
                minutes,
                CoreTempInput {
                    heart_rate: hr,
                    skin_temp: None,
                    heat_flux: None,
                },
            ) else {
                self.core_temp.remove(&crew);
                continue;
            };
            self.core_temp.insert(
                crew,
                CoreTrack {
                    filter,
                    device: track.device,
                    last_step: now.0,
                    last_hr,
                },
            );
            let o = self.store_derived(
                crew,
                track.device,
                Parameter::CoreTemp,
                est.value,
                est.quality,
                est.algorithm,
                now,
            )?;
            out.events.extend(o.events);
            out.notices.extend(o.notices);
        }
        Ok(out)
    }
}

/// Mean of the usable values among stored rows (good or degraded quality).
fn mean_of(rows: &[caelum_store::ObservationRecord]) -> Option<f64> {
    let v: Vec<f64> = rows
        .iter()
        .filter(|r| r.quality == "good" || r.quality == "degraded")
        .filter_map(|r| r.value)
        .collect();
    (!v.is_empty()).then(|| v.iter().sum::<f64>() / v.len() as f64)
}
