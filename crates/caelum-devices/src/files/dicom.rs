//! DICOM import (F-ACQ-06): images and ultrasound frames, plus DICOM waveforms such as 12-lead
//! ECG (F-ACQ-07), with dicom-rs (stack item, CR-014).
//!
//! Entry points:
//! - [`read_file`]: a DICOM Part 10 file (with or without the 128-byte preamble), from a file
//!   drop or the folder watch;
//! - [`read_received`]: a data set received over the network (the C-STORE SCP hands over the
//!   data set bytes and the transfer syntax negotiated for the presentation context).
//!
//! Transfer syntaxes (CR-014 §2): data sets in Implicit VR Little Endian, Explicit VR Little
//! Endian, Explicit VR Big Endian (retired) and Deflated Explicit VR Little Endian; pixel data
//! native, Encapsulated Uncompressed, RLE Lossless, JPEG Baseline (Process 1), JPEG Extended
//! (Process 2 & 4) and JPEG Lossless (Process 14, and SV1). JPEG 2000, JPEG-LS, JPEG XL, MPEG
//! and HEVC video need native or unsafe-heavy codecs and are not decoded: the object is still
//! imported (metadata, original kept by the caller) and its pixel result says why it cannot be
//! shown ([`PixelError::UnsupportedTransferSyntax`]).
//!
//! Pixel samples are returned exactly as decoded (no rescale, VOI or colour conversion is
//! applied); rescale and window values are returned alongside for the viewer.
//!
//! Waveforms (PS3.3 C.10.9): each multiplex group becomes a list of channels in physical units:
//! `(sample + Channel Baseline) × Channel Sensitivity × Channel Sensitivity Correction Factor`,
//! in the unit of Channel Sensitivity Units Sequence. A channel without sensitivity, correction
//! factor, baseline or unit is rejected (nothing is assumed); µ-law and A-law samples are not
//! decoded.

use dicom_core::header::Tag;
use dicom_core::PrimitiveValue;
use dicom_dictionary_std::tags;
use dicom_object::meta::FileMetaTableBuilder;
use dicom_object::{DefaultDicomObject, InMemDicomObject, OpenFileOptions};
use dicom_pixeldata::{PixelDecoder, PixelRepresentation};
use dicom_transfer_syntax_registry::{TransferSyntaxIndex, TransferSyntaxRegistry};

use super::{CivilDateTime, WaveformChannel};

/// An imported DICOM object.
#[derive(Debug, Clone, PartialEq)]
pub struct DicomImport {
    /// Identification and descriptive attributes.
    pub metadata: DicomMetadata,
    /// Decoded image frames; `Ok(None)` when the object has no Pixel Data.
    pub pixels: Result<Option<DicomFrames>, PixelError>,
    /// Waveform multiplex groups; empty when the object has no Waveform Sequence.
    pub waveforms: Result<Vec<WaveformGroup>, WaveformError>,
}

/// Identification and descriptive attributes. Type 2 and 3 attributes the object does not carry
/// (or carries empty) are `None`.
#[derive(Debug, Clone, PartialEq)]
pub struct DicomMetadata {
    /// Transfer syntax UID of the data set.
    pub transfer_syntax_uid: String,
    /// SOP Class UID.
    pub sop_class_uid: String,
    /// SOP Instance UID.
    pub sop_instance_uid: String,
    /// Study Instance UID.
    pub study_instance_uid: String,
    /// Series Instance UID.
    pub series_instance_uid: String,
    /// Modality (`US`, `ECG`, `DX`, …).
    pub modality: Option<String>,
    /// Patient ID.
    pub patient_id: Option<String>,
    /// Patient's Name (DICOM PN, `Family^Given`).
    pub patient_name: Option<String>,
    /// Study Date (DA) as written.
    pub study_date: Option<String>,
    /// Study Time (TM) as written.
    pub study_time: Option<String>,
    /// Timezone Offset From UTC (`+HHMM`), as written.
    pub timezone_offset: Option<String>,
    /// Study date and time when both are stated to the second; carries the UTC offset only if
    /// the object states one.
    pub study_datetime: Option<CivilDateTime>,
    /// Study Description.
    pub study_description: Option<String>,
    /// Series Description.
    pub series_description: Option<String>,
    /// Series Number.
    pub series_number: Option<i32>,
    /// Instance Number.
    pub instance_number: Option<i32>,
    /// Manufacturer.
    pub manufacturer: Option<String>,
    /// Manufacturer's Model Name.
    pub model_name: Option<String>,
    /// Device Serial Number.
    pub device_serial_number: Option<String>,
    /// Software Versions.
    pub software_versions: Option<String>,
}

/// Decoded image frames.
#[derive(Debug, Clone, PartialEq)]
pub struct DicomFrames {
    /// Rows.
    pub rows: u32,
    /// Columns.
    pub columns: u32,
    /// Samples per pixel (1 grey, 3 colour).
    pub samples_per_pixel: u16,
    /// Bits allocated per sample after decoding (1-bit data is expanded to 8: 0 or 255).
    pub bits_allocated: u16,
    /// Bits stored.
    pub bits_stored: u16,
    /// Whether samples are signed.
    pub signed: bool,
    /// Photometric interpretation after decoding (`MONOCHROME2`, `RGB`, …).
    pub photometric_interpretation: String,
    /// Colour samples stored plane by plane (Planar Configuration 1) rather than interleaved.
    pub planar: bool,
    /// One byte buffer per frame, samples little-endian.
    pub frames: Vec<Vec<u8>>,
    /// Rescale slope and intercept, if stated.
    pub rescale: Option<(f64, f64)>,
    /// Window centre and width pairs, if stated.
    pub windows: Vec<(f64, f64)>,
    /// Pixel Spacing (row, column), mm, if stated.
    pub pixel_spacing_mm: Option<(f64, f64)>,
    /// Frame Time (cine), ms, if stated.
    pub frame_time_ms: Option<f64>,
}

/// One waveform multiplex group.
#[derive(Debug, Clone, PartialEq)]
pub struct WaveformGroup {
    /// Multiplex Group Label (`RHYTHM`, …), if stated.
    pub label: Option<String>,
    /// Waveform Originality (`ORIGINAL` / `DERIVED`), if stated.
    pub originality: Option<String>,
    /// Sampling Frequency, Hz.
    pub sample_rate_hz: f64,
    /// Channels in physical units.
    pub channels: Vec<WaveformChannel>,
    /// Channel source codes, parallel to `channels`.
    pub sources: Vec<ChannelSource>,
}

/// Channel Source Sequence code (`CodeValue`, `CodingSchemeDesignator`, `CodeMeaning`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChannelSource {
    /// Code value (e.g. `5.6.3-9-1` in SCPECG, or an MDC code).
    pub code_value: Option<String>,
    /// Coding scheme designator.
    pub scheme: Option<String>,
    /// Code meaning (`Lead I`, …).
    pub meaning: Option<String>,
}

/// Errors that stop the import of an object.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DicomError {
    /// The bytes are not a readable DICOM object.
    #[error("not a readable DICOM object: {0}")]
    Parse(String),
    /// The transfer syntax UID is not known.
    #[error("unknown transfer syntax {0}")]
    UnknownTransferSyntax(String),
    /// The data set itself cannot be decoded in this transfer syntax.
    #[error("data set in transfer syntax {0} cannot be read")]
    UnsupportedTransferSyntax(String),
    /// A Type 1 identification attribute is missing or empty.
    #[error("required attribute {0} is missing")]
    MissingAttribute(&'static str),
}

/// Why pixel data could not be decoded.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum PixelError {
    /// The pixel data is compressed with a codec CAELUM does not include.
    #[error("pixel data in {name} ({uid}) is not decoded")]
    UnsupportedTransferSyntax {
        /// Transfer syntax UID.
        uid: String,
        /// Its name.
        name: String,
    },
    /// The pixel data or image attributes are invalid.
    #[error("invalid pixel data: {0}")]
    Invalid(String),
}

/// Why a waveform could not be read.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum WaveformError {
    /// A required waveform attribute is missing or malformed.
    #[error("waveform group {group}: {reason}")]
    Invalid {
        /// Multiplex group index (0-based).
        group: usize,
        /// What is wrong.
        reason: String,
    },
    /// A channel lacks what is needed to give its samples physical meaning.
    #[error("waveform group {group}, channel {channel}: {reason}")]
    Channel {
        /// Multiplex group index.
        group: usize,
        /// Channel index.
        channel: usize,
        /// What is missing.
        reason: &'static str,
    },
    /// Sample interpretation that is not decoded (µ-law `MB`, A-law `AB`).
    #[error("waveform group {group}: sample interpretation {interpretation} is not decoded")]
    Interpretation {
        /// Multiplex group index.
        group: usize,
        /// The interpretation code.
        interpretation: String,
    },
}

/// Imports a DICOM Part 10 file (F-ACQ-06, F-ACQ-07).
pub fn read_file(data: &[u8]) -> Result<DicomImport, DicomError> {
    let obj = OpenFileOptions::new()
        .from_reader(data)
        .map_err(|e| DicomError::Parse(e.to_string()))?;
    analyse(&obj)
}

/// Imports a data set received over the network in `transfer_syntax_uid` (the C-STORE SCP's
/// entry point; F-ACQ-06).
pub fn read_received(dataset: &[u8], transfer_syntax_uid: &str) -> Result<DicomImport, DicomError> {
    let uid = transfer_syntax_uid.trim_end_matches('\0').trim();
    let ts = TransferSyntaxRegistry
        .get(uid)
        .ok_or_else(|| DicomError::UnknownTransferSyntax(uid.to_owned()))?;
    if !ts.can_decode_dataset() {
        return Err(DicomError::UnsupportedTransferSyntax(uid.to_owned()));
    }
    let obj = InMemDicomObject::read_dataset_with_ts(dataset, ts)
        .map_err(|e| DicomError::Parse(e.to_string()))?;
    let obj = obj
        .with_meta(FileMetaTableBuilder::new().transfer_syntax(uid))
        .map_err(|e| DicomError::Parse(e.to_string()))?;
    analyse(&obj)
}

/// Whether pixel data in this transfer syntax is decoded by this build.
pub fn pixel_data_supported(transfer_syntax_uid: &str) -> bool {
    TransferSyntaxRegistry
        .get(transfer_syntax_uid)
        .is_some_and(|ts| ts.can_decode_all())
}

fn analyse(obj: &DefaultDicomObject) -> Result<DicomImport, DicomError> {
    let metadata = metadata(obj)?;
    let pixels = if obj.element_opt(tags::PIXEL_DATA).ok().flatten().is_some() {
        frames(obj).map(Some)
    } else {
        Ok(None)
    };
    let waveforms = waveforms(obj);
    Ok(DicomImport {
        metadata,
        pixels,
        waveforms,
    })
}

fn text(obj: &InMemDicomObject, tag: Tag) -> Option<String> {
    let e = obj.element_opt(tag).ok().flatten()?;
    let s = e.to_str().ok()?;
    let s = s.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    (!s.is_empty()).then(|| s.to_owned())
}

fn float(obj: &InMemDicomObject, tag: Tag) -> Option<f64> {
    text(obj, tag)?
        .split('\\')
        .next()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
}

fn floats(obj: &InMemDicomObject, tag: Tag) -> Option<Vec<f64>> {
    text(obj, tag)?
        .split('\\')
        .map(|v| v.trim().parse::<f64>().ok().filter(|v| v.is_finite()))
        .collect()
}

fn int(obj: &InMemDicomObject, tag: Tag) -> Option<i64> {
    let e = obj.element_opt(tag).ok().flatten()?;
    e.to_int::<i64>().ok()
}

fn required(obj: &InMemDicomObject, tag: Tag, name: &'static str) -> Result<String, DicomError> {
    text(obj, tag).ok_or(DicomError::MissingAttribute(name))
}

fn metadata(obj: &DefaultDicomObject) -> Result<DicomMetadata, DicomError> {
    let study_date = text(obj, tags::STUDY_DATE);
    let study_time = text(obj, tags::STUDY_TIME);
    let timezone_offset = text(obj, tags::TIMEZONE_OFFSET_FROM_UTC);
    let study_datetime = match (&study_date, &study_time) {
        (Some(d), Some(t)) => da_tm(d, t, timezone_offset.as_deref()),
        _ => None,
    };
    Ok(DicomMetadata {
        transfer_syntax_uid: obj
            .meta()
            .transfer_syntax()
            .trim_end_matches('\0')
            .to_owned(),
        sop_class_uid: required(obj, tags::SOP_CLASS_UID, "SOPClassUID")?,
        sop_instance_uid: required(obj, tags::SOP_INSTANCE_UID, "SOPInstanceUID")?,
        study_instance_uid: required(obj, tags::STUDY_INSTANCE_UID, "StudyInstanceUID")?,
        series_instance_uid: required(obj, tags::SERIES_INSTANCE_UID, "SeriesInstanceUID")?,
        modality: text(obj, tags::MODALITY),
        patient_id: text(obj, tags::PATIENT_ID),
        patient_name: text(obj, tags::PATIENT_NAME),
        study_date,
        study_time,
        timezone_offset,
        study_datetime,
        study_description: text(obj, tags::STUDY_DESCRIPTION),
        series_description: text(obj, tags::SERIES_DESCRIPTION),
        series_number: int(obj, tags::SERIES_NUMBER).and_then(|v| i32::try_from(v).ok()),
        instance_number: int(obj, tags::INSTANCE_NUMBER).and_then(|v| i32::try_from(v).ok()),
        manufacturer: text(obj, tags::MANUFACTURER),
        model_name: text(obj, tags::MANUFACTURER_MODEL_NAME),
        device_serial_number: text(obj, tags::DEVICE_SERIAL_NUMBER),
        software_versions: text(obj, tags::SOFTWARE_VERSIONS),
    })
}

/// DA `YYYYMMDD` + TM `HHMMSS[.F]` (+ `±HHMM`); `None` unless stated to the second.
fn da_tm(date: &str, time: &str, offset: Option<&str>) -> Option<CivilDateTime> {
    let digits = |s: &str| s.bytes().all(|c| c.is_ascii_digit());
    if date.len() != 8 || !digits(date) {
        return None;
    }
    let (main, frac) = time.split_once('.').unwrap_or((time, ""));
    if main.len() != 6 || !digits(main) || frac.len() > 9 || !digits(frac) {
        return None;
    }
    let nanos = if frac.is_empty() {
        0
    } else {
        format!("{frac:0<9}").parse().ok()?
    };
    let offset = match offset {
        None => None,
        Some(o)
            if o.len() == 5 && (o.starts_with('+') || o.starts_with('-')) && digits(&o[1..]) =>
        {
            let h: i16 = o[1..3].parse().ok()?;
            let m: i16 = o[3..5].parse().ok()?;
            Some(if o.starts_with('-') {
                -(h * 60 + m)
            } else {
                h * 60 + m
            })
        }
        Some(_) => return None,
    };
    CivilDateTime::new(
        date[0..4].parse().ok()?,
        date[4..6].parse().ok()?,
        date[6..8].parse().ok()?,
        main[0..2].parse().ok()?,
        main[2..4].parse().ok()?,
        main[4..6].parse().ok()?,
        nanos,
        offset,
    )
    .ok()
}

fn frames(obj: &DefaultDicomObject) -> Result<DicomFrames, PixelError> {
    let uid = obj.meta().transfer_syntax().trim_end_matches('\0');
    match TransferSyntaxRegistry.get(uid) {
        Some(ts) if ts.can_decode_all() => {}
        Some(ts) => {
            return Err(PixelError::UnsupportedTransferSyntax {
                uid: uid.to_owned(),
                name: ts.name().to_owned(),
            })
        }
        None => {
            return Err(PixelError::UnsupportedTransferSyntax {
                uid: uid.to_owned(),
                name: "unknown".to_owned(),
            })
        }
    }
    let decoded = obj
        .decode_pixel_data()
        .map_err(|e| PixelError::Invalid(e.to_string()))?;
    let (rows, columns) = (decoded.rows(), decoded.columns());
    let n_frames = decoded.number_of_frames();
    let spp = decoded.samples_per_pixel();
    let bits = decoded.bits_allocated();
    if rows == 0 || columns == 0 || n_frames == 0 || spp == 0 {
        return Err(PixelError::Invalid(
            "zero rows, columns, frames or samples".into(),
        ));
    }
    let (bytes_per_sample, bits_out) = match bits {
        1 => (1usize, 8u16), // expanded to one byte per pixel by the decoder
        8 => (1, 8),
        16 => (2, 16),
        32 => (4, 32),
        other => return Err(PixelError::Invalid(format!("bits allocated {other}"))),
    };
    let frame_len = (rows as usize)
        .checked_mul(columns as usize)
        .and_then(|n| n.checked_mul(usize::from(spp)))
        .and_then(|n| n.checked_mul(bytes_per_sample))
        .ok_or_else(|| PixelError::Invalid("image size overflows".into()))?;
    let data = decoded.data();
    let needed = frame_len
        .checked_mul(n_frames as usize)
        .ok_or_else(|| PixelError::Invalid("image size overflows".into()))?;
    if data.len() < needed {
        return Err(PixelError::Invalid(format!(
            "pixel data has {} bytes, the image attributes need {needed}",
            data.len()
        )));
    }
    let frames = data[..needed]
        .chunks_exact(frame_len)
        .map(<[u8]>::to_vec)
        .collect();
    let rescale = match (
        float(obj, tags::RESCALE_SLOPE),
        float(obj, tags::RESCALE_INTERCEPT),
    ) {
        (Some(s), Some(i)) => Some((s, i)),
        _ => None,
    };
    let windows = match (
        floats(obj, tags::WINDOW_CENTER),
        floats(obj, tags::WINDOW_WIDTH),
    ) {
        (Some(c), Some(w)) => c.into_iter().zip(w).collect(),
        _ => Vec::new(),
    };
    let pixel_spacing_mm = floats(obj, tags::PIXEL_SPACING).and_then(|v| match v.as_slice() {
        [r, c] => Some((*r, *c)),
        _ => None,
    });
    Ok(DicomFrames {
        rows,
        columns,
        samples_per_pixel: spp,
        bits_allocated: bits_out,
        bits_stored: decoded.bits_stored(),
        signed: decoded.pixel_representation() == PixelRepresentation::Signed,
        photometric_interpretation: decoded.photometric_interpretation().as_str().to_owned(),
        planar: spp > 1
            && decoded.planar_configuration() != dicom_pixeldata::PlanarConfiguration::Standard,
        frames,
        rescale,
        windows,
        pixel_spacing_mm,
        frame_time_ms: float(obj, tags::FRAME_TIME),
    })
}

fn waveforms(obj: &DefaultDicomObject) -> Result<Vec<WaveformGroup>, WaveformError> {
    let Some(seq) = obj.element_opt(tags::WAVEFORM_SEQUENCE).ok().flatten() else {
        return Ok(Vec::new());
    };
    let items = seq.items().ok_or(WaveformError::Invalid {
        group: 0,
        reason: "Waveform Sequence is not a sequence".into(),
    })?;
    items
        .iter()
        .enumerate()
        .map(|(g, item)| group(g, item))
        .collect()
}

fn group(g: usize, item: &InMemDicomObject) -> Result<WaveformGroup, WaveformError> {
    let invalid = |reason: &str| WaveformError::Invalid {
        group: g,
        reason: reason.to_owned(),
    };
    let n_channels = int(item, tags::NUMBER_OF_WAVEFORM_CHANNELS)
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("Number of Waveform Channels"))? as usize;
    let n_samples = int(item, tags::NUMBER_OF_WAVEFORM_SAMPLES)
        .filter(|n| *n > 0)
        .ok_or_else(|| invalid("Number of Waveform Samples"))? as usize;
    let rate = float(item, tags::SAMPLING_FREQUENCY)
        .filter(|f| *f > 0.0)
        .ok_or_else(|| invalid("Sampling Frequency"))?;
    let bits = int(item, tags::WAVEFORM_BITS_ALLOCATED)
        .ok_or_else(|| invalid("Waveform Bits Allocated"))?;
    let interpretation = text(item, tags::WAVEFORM_SAMPLE_INTERPRETATION)
        .ok_or_else(|| invalid("Waveform Sample Interpretation"))?;
    let bytes_per_sample = match (interpretation.as_str(), bits) {
        ("SS" | "US", 16) => 2usize,
        ("SB" | "UB", 8) => 1,
        ("MB" | "AB", _) => {
            return Err(WaveformError::Interpretation {
                group: g,
                interpretation,
            })
        }
        _ => {
            return Err(invalid(
                "sample interpretation does not match bits allocated",
            ))
        }
    };

    let defs = item
        .element_opt(tags::CHANNEL_DEFINITION_SEQUENCE)
        .ok()
        .flatten()
        .and_then(|e| e.items())
        .ok_or_else(|| invalid("Channel Definition Sequence"))?;
    if defs.len() != n_channels {
        return Err(invalid(
            "Channel Definition Sequence length differs from the channel count",
        ));
    }

    let data = item
        .element_opt(tags::WAVEFORM_DATA)
        .ok()
        .flatten()
        .ok_or_else(|| invalid("no Waveform Data"))?;
    let bytes: Vec<u8> = match data.value().primitive() {
        Some(PrimitiveValue::U8(v)) => v.to_vec(),
        Some(PrimitiveValue::U16(v)) => v.iter().flat_map(|w| w.to_le_bytes()).collect(),
        Some(PrimitiveValue::I16(v)) => v.iter().flat_map(|w| w.to_le_bytes()).collect(),
        _ => return Err(invalid("Waveform Data is not OB/OW")),
    };
    let needed = n_channels * n_samples * bytes_per_sample;
    if bytes.len() < needed {
        return Err(invalid("Waveform Data is shorter than channels × samples"));
    }
    let raw = |s: usize, c: usize| -> f64 {
        let i = (s * n_channels + c) * bytes_per_sample;
        match interpretation.as_str() {
            "SS" => f64::from(i16::from_le_bytes([bytes[i], bytes[i + 1]])),
            "US" => f64::from(u16::from_le_bytes([bytes[i], bytes[i + 1]])),
            "SB" => f64::from(bytes[i] as i8),
            _ => f64::from(bytes[i]),
        }
    };

    let mut channels = Vec::with_capacity(n_channels);
    let mut sources = Vec::with_capacity(n_channels);
    for (c, def) in defs.iter().enumerate() {
        let missing = |reason| WaveformError::Channel {
            group: g,
            channel: c,
            reason,
        };
        let source_item = def
            .element_opt(tags::CHANNEL_SOURCE_SEQUENCE)
            .ok()
            .flatten()
            .and_then(|e| e.items())
            .and_then(|i| i.first());
        let source = source_item
            .map(|s| ChannelSource {
                code_value: text(s, tags::CODE_VALUE),
                scheme: text(s, tags::CODING_SCHEME_DESIGNATOR),
                meaning: text(s, tags::CODE_MEANING),
            })
            .unwrap_or_default();
        let sensitivity = float(def, tags::CHANNEL_SENSITIVITY).ok_or(missing(
            "no Channel Sensitivity: samples have no physical meaning",
        ))?;
        let correction = float(def, tags::CHANNEL_SENSITIVITY_CORRECTION_FACTOR)
            .ok_or(missing("no Channel Sensitivity Correction Factor"))?;
        let baseline = float(def, tags::CHANNEL_BASELINE).ok_or(missing("no Channel Baseline"))?;
        let unit = def
            .element_opt(tags::CHANNEL_SENSITIVITY_UNITS_SEQUENCE)
            .ok()
            .flatten()
            .and_then(|e| e.items())
            .and_then(|i| i.first())
            .and_then(|u| text(u, tags::CODE_VALUE))
            .ok_or(missing("no Channel Sensitivity Units"))?;
        let label = text(def, tags::CHANNEL_LABEL)
            .or_else(|| source.meaning.clone())
            .or_else(|| source.code_value.clone())
            .ok_or(missing("channel has neither label nor source code"))?;
        let gain = sensitivity * correction;
        channels.push(WaveformChannel {
            label,
            unit,
            sample_rate_hz: rate,
            samples: (0..n_samples)
                .map(|s| (raw(s, c) + baseline) * gain)
                .collect(),
        });
        sources.push(source);
    }
    Ok(WaveformGroup {
        label: text(item, tags::MULTIPLEX_GROUP_LABEL),
        originality: text(item, tags::WAVEFORM_ORIGINALITY),
        sample_rate_hz: rate,
        channels,
        sources,
    })
}
