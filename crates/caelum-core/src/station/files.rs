//! Clinical file import and review (M8 integration; F-ACQ-06, F-ACQ-07, F-ACQ-08, F-CLN-03,
//! F-CLN-04; CR-014, CR-019): imported files are parsed before they are kept, so only a file
//! that opens is stored (encrypted, with what was found in it); the viewers read it back
//! decoded. FHIR observations are stored as `imported` and evaluated like any reading (SR-016).

use caelum_devices::files::dicom::DicomFrames;
use caelum_devices::files::{aecg, dicom, edf, fhir, scp_ecg, WaveformChannel};
use caelum_devices::registry::{DeviceIdentity, Registry};
use caelum_domain::ids::CrewMemberId;
use caelum_domain::observation::Classification;
use caelum_domain::time::LeapSeconds;
use caelum_security::audit::{Actor, AuditAction};
use caelum_security::role::Command;
use serde_json::json;

use super::{Station, StationError};
use crate::acquisition::observation_record;
use crate::alarms::Output;

fn rejected(reason: impl Into<String>) -> StationError {
    StationError::Rejected(reason.into())
}

/// Leads of an ECG recording, decoded.
#[derive(Debug, Clone, PartialEq)]
pub struct EcgRecording {
    /// Format (`edf`, `aecg`, `scp-ecg`, `dicom-waveform`).
    pub format: &'static str,
    /// Leads in physical units.
    pub leads: Vec<WaveformChannel>,
    /// Start of the recording as stated in the file; `None` if the file does not say.
    pub start: Option<caelum_devices::files::CivilDateTime>,
    /// Events stated in the file (EDF+ annotations), for review (F-CLN-03).
    pub events: Vec<EcgEvent>,
}

/// One event stated in an ECG file.
#[derive(Debug, Clone, PartialEq)]
pub struct EcgEvent {
    /// Seconds after the recording start.
    pub onset_s: f64,
    /// Duration, s, if stated.
    pub duration_s: Option<f64>,
    /// Text.
    pub text: String,
}

/// One DICOM frame rendered for display (F-CLN-04): 8-bit samples, interleaved, row by row.
#[derive(Debug, Clone, PartialEq)]
pub struct DicomView {
    /// Rows.
    pub rows: u32,
    /// Columns.
    pub columns: u32,
    /// Frames in the object.
    pub frames: u32,
    /// The frame shown (0-based).
    pub frame: u32,
    /// 1 (grey) or 3 (RGB).
    pub channels: u8,
    /// Photometric interpretation of the stored data.
    pub photometric: String,
    /// Display samples, `rows * columns * channels`.
    pub pixels: Vec<u8>,
    /// Window centre and width applied (grey only), in rescaled units.
    pub window: Option<(f64, f64)>,
    /// Window presets stated in the file.
    pub presets: Vec<(f64, f64)>,
    /// Pixel spacing (row, column) in mm, if stated; calipers are offered only with it.
    pub pixel_spacing_mm: Option<(f64, f64)>,
}

/// Renders one frame for display. Grey data are rescaled and windowed (DICOM PS3.3 C.11.2.1.2,
/// linear); without a window the frame's own value range is shown, so nothing is assumed.
pub fn render_frame(
    f: &DicomFrames,
    frame: usize,
    window: Option<(f64, f64)>,
) -> Result<DicomView, String> {
    let data = f.frames.get(frame).ok_or("no such frame")?;
    let n = f.rows as usize * f.columns as usize;
    let base = DicomView {
        rows: f.rows,
        columns: f.columns,
        frames: u32::try_from(f.frames.len()).map_err(|_| "too many frames")?,
        frame: u32::try_from(frame).map_err(|_| "no such frame")?,
        channels: 1,
        photometric: f.photometric_interpretation.clone(),
        pixels: Vec::new(),
        window: None,
        presets: f.windows.clone(),
        pixel_spacing_mm: f.pixel_spacing_mm,
    };
    match (f.photometric_interpretation.as_str(), f.samples_per_pixel) {
        ("RGB", 3) => {
            if f.bits_allocated != 8 || data.len() < n * 3 {
                return Err("only 8-bit RGB is shown".to_owned());
            }
            let pixels = if f.planar {
                (0..n)
                    .flat_map(|i| [data[i], data[n + i], data[2 * n + i]])
                    .collect()
            } else {
                data[..n * 3].to_vec()
            };
            Ok(DicomView {
                channels: 3,
                pixels,
                ..base
            })
        }
        (p @ ("MONOCHROME1" | "MONOCHROME2"), 1) => {
            let values = grey_values(f, data, n)?;
            let window = match window.or_else(|| f.windows.first().copied()) {
                Some(w) => w,
                None => {
                    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
                    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                    let width = (hi - lo).max(0.0) + 1.0;
                    (lo + width / 2.0, width)
                }
            };
            let (c, w) = window;
            let lo = c - 0.5 - (w - 1.0) / 2.0;
            let hi = c - 0.5 + (w - 1.0) / 2.0;
            let invert = p == "MONOCHROME1";
            let pixels = values
                .iter()
                .map(|&x| {
                    let y = if x <= lo {
                        0.0
                    } else if x > hi {
                        255.0
                    } else {
                        ((x - (c - 0.5)) / (w - 1.0).max(1.0) + 0.5) * 255.0
                    };
                    // y is clamped to 0..=255 above; the cast cannot overflow.
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let v = y.round().clamp(0.0, 255.0) as u8;
                    if invert {
                        255 - v
                    } else {
                        v
                    }
                })
                .collect();
            Ok(DicomView {
                pixels,
                window: Some(window),
                ..base
            })
        }
        (p, spp) => Err(format!("{p} with {spp} samples per pixel is not shown")),
    }
}

fn grey_values(f: &DicomFrames, data: &[u8], n: usize) -> Result<Vec<f64>, String> {
    let bits = u32::from(f.bits_stored.clamp(1, 16));
    let raw: Vec<i64> = match f.bits_allocated {
        8 if data.len() >= n => data[..n].iter().map(|&b| i64::from(b)).collect(),
        16 if data.len() >= n * 2 => data[..n * 2]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|c| i64::from(u16::from_le_bytes(*c)))
            .collect(),
        8 | 16 => return Err("the frame is shorter than its size".to_owned()),
        b => return Err(format!("{b}-bit samples are not shown")),
    };
    let mask = (1i64 << bits) - 1;
    let (slope, intercept) = f.rescale.unwrap_or((1.0, 0.0));
    Ok(raw
        .into_iter()
        .map(|v| {
            let v = v & mask;
            let v = if f.signed && v >= 1 << (bits - 1) {
                v - (1 << bits)
            } else {
                v
            };
            // Stored values are at most 16 bits; f64 holds them exactly.
            #[allow(clippy::cast_precision_loss)]
            let x = v as f64;
            x * slope + intercept
        })
        .collect())
}

/// Decodes an ECG file by its content: EDF/EDF+, HL7 aECG, SCP-ECG or a DICOM waveform.
pub fn decode_ecg(bytes: &[u8]) -> Result<EcgRecording, String> {
    if bytes.len() > 132 && &bytes[128..132] == b"DICM" {
        let d = dicom::read_file(bytes).map_err(|e| e.to_string())?;
        let groups = d.waveforms.map_err(|e| e.to_string())?;
        let leads: Vec<WaveformChannel> = groups.into_iter().flat_map(|g| g.channels).collect();
        return non_empty("dicom-waveform", leads);
    }
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]);
    if head.contains("AnnotatedECG") {
        let doc = aecg::read(bytes).map_err(|e| e.to_string())?;
        let leads: Vec<WaveformChannel> = doc.series.into_iter().flat_map(|s| s.leads).collect();
        return non_empty("aecg", leads);
    }
    if bytes.first() == Some(&b'0') && bytes.len() >= 256 {
        let f = edf::read(bytes).map_err(|e| e.to_string())?;
        let leads: Vec<WaveformChannel> = f.signals.iter().map(|s| s.to_channel()).collect();
        let events = f
            .annotations
            .iter()
            .map(|a| EcgEvent {
                onset_s: a.onset_s,
                duration_s: a.duration_s,
                text: a.text.clone(),
            })
            .collect();
        return with_context("edf", leads, Some(f.start), events);
    }
    let scp = scp_ecg::read(bytes).map_err(|e| format!("not a supported ECG file: {e}"))?;
    with_context("scp-ecg", scp.leads, scp.acquired, Vec::new())
}

fn non_empty(format: &'static str, leads: Vec<WaveformChannel>) -> Result<EcgRecording, String> {
    with_context(format, leads, None, Vec::new())
}

fn with_context(
    format: &'static str,
    leads: Vec<WaveformChannel>,
    start: Option<caelum_devices::files::CivilDateTime>,
    events: Vec<EcgEvent>,
) -> Result<EcgRecording, String> {
    if leads.is_empty()
        || leads
            .iter()
            .any(|l| l.samples.is_empty() || l.sample_rate_hz.is_nan() || l.sample_rate_hz <= 0.0)
    {
        return Err("the file holds no usable lead".to_owned());
    }
    Ok(EcgRecording {
        format,
        leads,
        start,
        events,
    })
}

impl Station {
    /// Imports a clinical file: a DICOM object or ECG recording must open before it is kept
    /// (CR-014); its identity goes into the attachment metadata. Other files are kept as they are.
    pub fn file_import(
        &mut self,
        crew: CrewMemberId,
        kind: &str,
        filename: &str,
        bytes: &[u8],
    ) -> Result<String, StationError> {
        let metadata = match kind {
            "dicom" => {
                let d = dicom::read_file(bytes)
                    .map_err(|e| rejected(format!("the DICOM file does not open: {e}")))?;
                let m = &d.metadata;
                let frames = match &d.pixels {
                    Ok(Some(f)) => {
                        json!({ "rows": f.rows, "columns": f.columns, "frames": f.frames.len() })
                    }
                    Ok(None) => json!(null),
                    Err(e) => json!({ "not_shown": e.to_string() }),
                };
                json!({
                    "sop_class_uid": m.sop_class_uid, "sop_instance_uid": m.sop_instance_uid,
                    "study_instance_uid": m.study_instance_uid, "modality": m.modality,
                    "study_date": m.study_date, "manufacturer": m.manufacturer, "model": m.model_name,
                    "transfer_syntax": m.transfer_syntax_uid, "pixels": frames,
                })
            }
            "ecg" => {
                let r = decode_ecg(bytes)
                    .map_err(|e| rejected(format!("the ECG file does not open: {e}")))?;
                let rate = r.leads.first().map(|l| l.sample_rate_hz);
                let seconds = r
                    .leads
                    .first()
                    .map(|l| l.samples.len() as f64 / l.sample_rate_hz);
                json!({
                    "format": r.format,
                    "leads": r.leads.iter().map(|l| l.label.clone()).collect::<Vec<_>>(),
                    "sample_rate_hz": rate, "duration_s": seconds,
                })
            }
            _ => json!({ "size": bytes.len() }),
        };
        self.attachment_import(crew, kind, filename, bytes, metadata)
    }

    /// `ecg_view` (F-CLN-03): an imported ECG recording, decoded for review.
    pub fn ecg_view(&mut self, id: &str) -> Result<EcgRecording, StationError> {
        self.user_for(Command::EcgView)?;
        let (record, bytes) = self.attachment_get(id)?;
        if record.kind != "ecg" {
            return Err(rejected("not an ECG recording"));
        }
        decode_ecg(&bytes).map_err(rejected)
    }

    /// `dicom_view` (F-CLN-04): one frame of an imported DICOM image, rendered for the viewer
    /// with the given window (or the file's first, or the frame's own range).
    pub fn dicom_view(
        &mut self,
        id: &str,
        frame: u32,
        window: Option<(f64, f64)>,
    ) -> Result<DicomView, StationError> {
        self.user_for(Command::DicomView)?;
        let (record, bytes) = self.attachment_get(id)?;
        if record.kind != "dicom" {
            return Err(rejected("not a DICOM object"));
        }
        let d = dicom::read_file(&bytes).map_err(|e| rejected(e.to_string()))?;
        let f = d
            .pixels
            .map_err(|e| rejected(format!("the image cannot be shown: {e}")))?
            .ok_or_else(|| rejected("the DICOM object holds no image"))?;
        render_frame(&f, frame as usize, window).map_err(rejected)
    }

    /// `fhir_import` (F-ACQ-08): observations from a FHIR R4 resource or bundle, for one crew
    /// member, stored as `imported` and evaluated (SR-016). Returns how many were imported, what
    /// was not and why, and the alarm output to deliver.
    pub fn fhir_import(
        &mut self,
        crew: CrewMemberId,
        filename: &str,
        json_bytes: &[u8],
    ) -> Result<(usize, Vec<String>, Output), StationError> {
        let user = self.user_for(Command::FhirImport)?;
        if !self
            .db
            .crew_member(&crew.to_string())?
            .is_some_and(|c| c.active)
        {
            return Err(rejected("the crew member is not on the roster in force"));
        }
        let leap = LeapSeconds::bundled().map_err(|e| rejected(e.to_string()))?;
        let result =
            fhir::import(json_bytes, &leap).map_err(|e| rejected(format!("not FHIR JSON: {e}")))?;
        let device = Registry::new(&self.db)
            .register(&DeviceIdentity {
                adapter: "fhir-import".to_owned(),
                vendor: "FHIR import".to_owned(),
                model: "file".to_owned(),
                serial: "fhir-import".to_owned(),
                firmware: None,
                accuracy_class: None,
                calibration_due: None,
            })
            .map_err(|e| rejected(e.to_string()))?;
        let now = self.now()?;
        let mut out = Output::default();
        let mut n = 0;
        let mut refused: Vec<String> = result
            .rejected
            .iter()
            .map(|r| format!("{}: {}", r.location, r.reason))
            .collect();
        for o in result.observations {
            // Through the normal ingest checks (unit, plausibility, time, SR-017); FHIR states
            // no signal quality, so the reading is degraded, never assumed good.
            let mut obs =
                match caelum_ingest::ingest(o.to_raw_reading(device, now), Some(crew), None) {
                    Ok(obs) => obs,
                    Err(e) => {
                        refused.push(format!("{}: {e}", o.location));
                        continue;
                    }
                };
            if obs.classification == Classification::Measured {
                obs.classification = Classification::Imported;
            }
            let row = observation_record(&obs).map_err(|e| rejected(e.to_string()))?;
            self.db.insert_observation(&row)?;
            let o2 = self.observe(&obs)?;
            out.events.extend(o2.events);
            out.notices.extend(o2.notices);
            n += 1;
        }
        self.log().append(
            &Actor::user(&user),
            AuditAction::FhirImported,
            Some(&crew.to_string()),
            &json!({ "imported": n, "rejected": refused.len() }),
        )?;
        // The original is kept, encrypted, with what came of it (CR-014 §1).
        self.attachment_import(
            crew,
            "file",
            filename,
            json_bytes,
            json!({ "format": "fhir", "imported": n, "rejected": refused.len() }),
        )?;
        Ok((n, refused, out))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // test code
mod tests {
    use super::*;

    fn frames(photometric: &str, spp: u16, bits: u16, signed: bool, data: Vec<u8>) -> DicomFrames {
        DicomFrames {
            rows: 1,
            columns: 4,
            samples_per_pixel: spp,
            bits_allocated: bits,
            bits_stored: bits,
            signed,
            photometric_interpretation: photometric.to_owned(),
            planar: false,
            frames: vec![data],
            rescale: None,
            windows: vec![],
            pixel_spacing_mm: None,
            frame_time_ms: None,
        }
    }

    #[test]
    fn f_cln_04_without_a_window_the_frames_own_range_is_shown() {
        let v = render_frame(
            &frames("MONOCHROME2", 1, 8, false, vec![10, 20, 30, 40]),
            0,
            None,
        )
        .unwrap();
        assert_eq!(v.pixels.first(), Some(&0));
        assert_eq!(v.pixels.last(), Some(&255));
        assert!(v.pixels.windows(2).all(|p| p[0] < p[1]));
        assert_eq!((v.channels, v.frames, v.frame), (1, 1, 0));
    }

    #[test]
    fn f_cln_04_the_window_follows_dicom_c_11_2_1_2_and_monochrome1_is_inverted() {
        let f = frames("MONOCHROME2", 1, 8, false, vec![0, 99, 101, 255]);
        // Centre 100.5, width 2: below 100 → black, above 100 → white.
        let v = render_frame(&f, 0, Some((100.5, 2.0))).unwrap();
        assert_eq!(v.pixels, vec![0, 0, 255, 255]);
        let mut f1 = f.clone();
        f1.photometric_interpretation = "MONOCHROME1".to_owned();
        assert_eq!(
            render_frame(&f1, 0, Some((100.5, 2.0))).unwrap().pixels,
            vec![255, 255, 0, 0]
        );
        assert!(render_frame(&f, 1, None).is_err());
    }

    #[test]
    fn f_cln_04_signed_16_bit_samples_and_rescale_are_honoured() {
        // -1000, 0, 1000, 2000 as signed 16-bit little-endian; rescale ×1 −1024.
        let raw: Vec<u8> = [-1000_i16, 0, 1000, 2000]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let mut f = frames("MONOCHROME2", 1, 16, true, raw);
        f.rescale = Some((1.0, -1024.0));
        // Window centred on 0 HU-like units after rescale: only 1000 and 2000 (→ −24, 976) lie above.
        let v = render_frame(&f, 0, Some((0.0, 2.0))).unwrap();
        assert_eq!(v.pixels, vec![0, 0, 0, 255]);
    }

    #[test]
    fn f_cln_04_rgb_planes_are_interleaved_and_other_colour_models_are_refused() {
        let mut f = frames(
            "RGB",
            3,
            8,
            false,
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
        );
        f.planar = true;
        let v = render_frame(&f, 0, None).unwrap();
        assert_eq!(v.channels, 3);
        assert_eq!(&v.pixels[..6], &[1, 5, 9, 2, 6, 10]);
        let ybr = frames("YBR_FULL", 3, 8, false, vec![0; 12]);
        assert!(render_frame(&ybr, 0, None).is_err());
    }

    #[test]
    fn f_acq_07_unknown_bytes_are_not_an_ecg() {
        assert!(decode_ecg(b"plain text").is_err());
        assert!(decode_ecg(b"").is_err());
    }
}
