//! M8 gate (F-ACQ-06, F-ACQ-07, F-ACQ-08, F-SYS-08; CR-014): reference files open and measure
//! correctly, EDF+ and CSV exports reopen, FHIR import follows the catalog (no unit → rejected,
//! LOINC 2708-6 not taken as SpO₂). Reference files are built here from known values (test
//! fixtures, never product data).

#![allow(clippy::expect_used, clippy::unwrap_used)] // test code

use caelum_devices::files::csv::{self, ExportRow, ExportValue};
use caelum_devices::files::edf::{
    self, EdfAnnotation, EdfPlusPatient, EdfPlusRecordingId, EdfPlusWrite, EdfVariant,
    EdfWriteSignal,
};
use caelum_devices::files::fhir::{self, FhirRejection};
use caelum_devices::files::{aecg, CivilDateTime};
use caelum_domain::observation::{Classification, Quality};
use caelum_domain::parameters::Parameter;
use caelum_domain::time::{LeapSeconds, TaiNs};
use caelum_domain::units::Unit;

fn leap() -> LeapSeconds {
    LeapSeconds::bundled().unwrap()
}

fn start() -> CivilDateTime {
    CivilDateTime {
        year: 2026,
        month: 10,
        day: 10,
        hour: 12,
        minute: 0,
        second: 0,
        nanosecond: 0,
        utc_offset_minutes: None,
    }
}

/// A 2 s, 250 Hz test ECG: 1 mV R waves at 60 bpm on a flat line.
fn ecg() -> Vec<f64> {
    (0..500)
        .map(|i| if i % 250 == 125 { 1.0 } else { 0.0 })
        .collect()
}

#[test]
fn f_sys_08_f_acq_07_edf_plus_export_reopens_with_the_same_samples() {
    let samples = ecg();
    let w = EdfPlusWrite {
        patient: EdfPlusPatient {
            code: Some("CREW-A".into()),
            sex: None,
            birthdate: None,
            name: None,
        },
        recording: EdfPlusRecordingId::default(),
        start: start(),
        record_duration_s: 1.0,
        signals: vec![EdfWriteSignal::fitted("ECG II", "mV", samples.clone(), 250).unwrap()],
        annotations: vec![EdfAnnotation {
            onset_s: 1.0,
            duration_s: None,
            text: "event".into(),
        }],
    };
    let bytes = edf::write_edf_plus(&w).unwrap();
    let f = edf::read(&bytes).unwrap();
    assert!(matches!(f.variant, EdfVariant::EdfPlusContinuous));
    assert_eq!(f.signals.len(), 1);
    let s = &f.signals[0];
    assert_eq!(
        (
            s.label.as_str(),
            s.physical_dimension.as_str(),
            s.sample_rate_hz
        ),
        ("ECG II", "mV", 250.0)
    );
    assert_eq!(s.samples.len(), samples.len());
    // 16-bit resolution over the fitted range: every sample within one digital step.
    let step = (s.physical_max - s.physical_min) / 65_535.0;
    assert!(s
        .samples
        .iter()
        .zip(&samples)
        .all(|(a, b)| (a - b).abs() <= step));
    assert!(f
        .annotations
        .iter()
        .any(|a| a.text == "event" && (a.onset_s - 1.0).abs() < 1e-9));
}

#[test]
fn f_acq_07_truncated_or_garbage_edf_is_refused() {
    let w = EdfPlusWrite {
        patient: EdfPlusPatient {
            code: None,
            sex: None,
            birthdate: None,
            name: None,
        },
        recording: EdfPlusRecordingId::default(),
        start: start(),
        record_duration_s: 1.0,
        signals: vec![EdfWriteSignal::fitted("ECG II", "mV", ecg(), 250).unwrap()],
        annotations: vec![],
    };
    let bytes = edf::write_edf_plus(&w).unwrap();
    assert!(edf::read(&bytes[..bytes.len() - 7]).is_err());
    assert!(edf::read(b"not an edf file").is_err());
}

#[test]
fn f_sys_08_csv_export_follows_rfc_4180_and_parses_back() {
    let rows = vec![
        ExportRow {
            parameter: Parameter::HeartRate,
            measured_at: TaiNs(2_107_382_437_000_000_000),
            value: ExportValue::Number(72.0),
            unit: Some(Unit::PerMinute),
            quality: Quality::Good,
            classification: Classification::Measured,
            device_id: None,
        },
        ExportRow {
            parameter: Parameter::HeartRate,
            measured_at: TaiNs(2_107_382_438_000_000_000),
            value: ExportValue::Number(74.5),
            unit: Some(Unit::PerMinute),
            quality: Quality::Degraded,
            classification: Classification::Derived,
            device_id: None,
        },
    ];
    let text = csv::export_parameter(Parameter::HeartRate, &rows, &leap()).unwrap();
    assert!(text.ends_with("\r\n"));
    let parsed = csv::parse(&text).unwrap();
    assert_eq!(parsed.len(), 3, "header and two rows");
    assert!(parsed[1].iter().any(|c| c == "72"));
    assert!(parsed[2].iter().any(|c| c == "74.5"));
    // A row of another parameter is refused, never written into this file.
    let mut wrong = rows[0].clone();
    wrong.parameter = Parameter::Spo2;
    assert!(csv::export_parameter(Parameter::HeartRate, &[wrong], &leap()).is_err());
    // Quoting: a field with a comma, quote or line break.
    assert_eq!(
        csv::parse("a,\"b,c\",\"say \"\"hi\"\"\"\r\n").unwrap(),
        vec![vec!["a".to_owned(), "b,c".into(), "say \"hi\"".into()]]
    );
}

fn observation(code: &str, value: &str) -> String {
    format!(
        r#"{{"resourceType":"Observation","id":"o1","status":"final",
            "code":{{"coding":[{{"system":"http://loinc.org","code":"{code}"}}]}},
            "subject":{{"reference":"Patient/p1"}},
            "effectiveDateTime":"2026-10-10T12:00:00Z",
            "valueQuantity":{value}}}"#
    )
}

#[test]
fn f_acq_08_fhir_observations_map_by_catalog_loinc_only() {
    let hr = observation(
        "8867-4",
        r#"{"value":72,"unit":"beats/minute","system":"http://unitsofmeasure.org","code":"/min"}"#,
    );
    let r = fhir::import(hr.as_bytes(), &leap()).unwrap();
    assert_eq!(r.observations.len(), 1, "{:?}", r.rejected);
    assert_eq!(r.observations[0].parameter, Parameter::HeartRate);
    assert_eq!(r.observations[0].value, 72.0);

    // No unit: rejected, never assumed.
    let no_unit = observation("8867-4", r#"{"value":72}"#);
    let r = fhir::import(no_unit.as_bytes(), &leap()).unwrap();
    assert!(r.observations.is_empty());
    assert!(
        matches!(
            r.rejected[0].reason,
            FhirRejection::MissingUnit | FhirRejection::UnitNotUcum { .. }
        ),
        "{:?}",
        r.rejected
    );

    // LOINC 2708-6 (arterial O2 saturation) is not the catalog's SpO2 code: not mapped.
    let sao2 = observation(
        "2708-6",
        r#"{"value":97,"unit":"%","system":"http://unitsofmeasure.org","code":"%"}"#,
    );
    let r = fhir::import(sao2.as_bytes(), &leap()).unwrap();
    assert!(r
        .observations
        .iter()
        .all(|o| o.parameter != Parameter::Spo2));
    assert!(
        matches!(r.rejected[0].reason, FhirRejection::UnknownLoincCode(_)),
        "{:?}",
        r.rejected
    );

    // A bundle: each resource on its own; one bad entry does not hide the good one.
    let bundle = format!(
        r#"{{"resourceType":"Bundle","type":"collection","entry":[{{"resource":{hr}}},{{"resource":{no_unit}}}]}}"#
    );
    let r = fhir::import(bundle.as_bytes(), &leap()).unwrap();
    assert_eq!((r.observations.len(), r.rejected.len()), (1, 1));
    assert!(fhir::import(b"{not json", &leap()).is_err());
}

#[test]
fn f_acq_07_hl7_aecg_leads_open_with_their_rate_and_values() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<AnnotatedECG xmlns="urn:hl7-org:v3">
  <id root="1.2.3"/>
  <component><series>
    <code code="RHYTHM" codeSystem="2.16.840.1.113883.5.4"/>
    <component><sequenceSet>
      <component><sequence>
        <code code="TIME_ABSOLUTE" codeSystem="2.16.840.1.113883.5.4"/>
        <value xsi:type="GLIST_TS" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
          <head value="20261010120000.000"/><increment value="0.004" unit="s"/>
        </value>
      </sequence></component>
      <component><sequence>
        <code code="MDC_ECG_LEAD_II" codeSystem="2.16.840.1.113883.6.24"/>
        <value xsi:type="SLIST_PQ" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
          <origin value="0" unit="uV"/><scale value="5" unit="uV"/>
          <digits>0 0 200 0 0</digits>
        </value>
      </sequence></component>
    </sequenceSet></component>
  </series></component>
</AnnotatedECG>"#;
    let doc = aecg::read(xml.as_bytes()).unwrap();
    assert_eq!(doc.series.len(), 1);
    let s = &doc.series[0];
    assert!((s.sample_rate_hz - 250.0).abs() < 1e-9);
    assert_eq!(s.leads.len(), 1);
    // 200 digits × 5 uV = 1000 uV.
    let peak = s.leads[0].samples.iter().copied().fold(f64::MIN, f64::max);
    assert!(peak > 0.0, "{:?}", s.leads[0]);
    assert!(aecg::read(b"<notecg/>").is_err());
}

/// A 4×4, 8-bit monochrome Secondary Capture image built with dicom-rs.
fn dicom_image() -> Vec<u8> {
    use dicom_core::{DataElement, PrimitiveValue, VR};
    use dicom_dictionary_std::tags;
    use dicom_object::{FileMetaTableBuilder, InMemDicomObject};
    let sop_class = "1.2.840.10008.5.1.4.1.1.7";
    let sop_instance = "1.2.826.0.1.3680043.8.498.1";
    let pixels: Vec<u8> = (0..16).map(|i| i * 16).collect();
    let obj = InMemDicomObject::from_element_iter([
        DataElement::new(tags::SOP_CLASS_UID, VR::UI, PrimitiveValue::from(sop_class)),
        DataElement::new(
            tags::SOP_INSTANCE_UID,
            VR::UI,
            PrimitiveValue::from(sop_instance),
        ),
        DataElement::new(
            tags::STUDY_INSTANCE_UID,
            VR::UI,
            PrimitiveValue::from("1.2.826.0.1.3680043.8.498.2"),
        ),
        DataElement::new(
            tags::SERIES_INSTANCE_UID,
            VR::UI,
            PrimitiveValue::from("1.2.826.0.1.3680043.8.498.3"),
        ),
        DataElement::new(tags::MODALITY, VR::CS, PrimitiveValue::from("US")),
        DataElement::new(tags::PATIENT_ID, VR::LO, PrimitiveValue::from("CREW-A")),
        DataElement::new(tags::SAMPLES_PER_PIXEL, VR::US, PrimitiveValue::from(1_u16)),
        DataElement::new(
            tags::PHOTOMETRIC_INTERPRETATION,
            VR::CS,
            PrimitiveValue::from("MONOCHROME2"),
        ),
        DataElement::new(tags::ROWS, VR::US, PrimitiveValue::from(4_u16)),
        DataElement::new(tags::COLUMNS, VR::US, PrimitiveValue::from(4_u16)),
        DataElement::new(tags::BITS_ALLOCATED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::BITS_STORED, VR::US, PrimitiveValue::from(8_u16)),
        DataElement::new(tags::HIGH_BIT, VR::US, PrimitiveValue::from(7_u16)),
        DataElement::new(
            tags::PIXEL_REPRESENTATION,
            VR::US,
            PrimitiveValue::from(0_u16),
        ),
        DataElement::new(tags::PIXEL_DATA, VR::OB, PrimitiveValue::from(pixels)),
    ]);
    let file = obj
        .with_meta(
            FileMetaTableBuilder::new()
                .transfer_syntax("1.2.840.10008.1.2.1")
                .media_storage_sop_class_uid(sop_class)
                .media_storage_sop_instance_uid(sop_instance),
        )
        .unwrap();
    let mut out = Vec::new();
    file.write_all(&mut out).unwrap();
    out
}

#[test]
fn f_acq_06_a_dicom_image_opens_with_its_identity_and_pixels() {
    let bytes = dicom_image();
    let d = caelum_devices::files::dicom::read_file(&bytes).unwrap();
    assert_eq!(d.metadata.sop_class_uid, "1.2.840.10008.5.1.4.1.1.7");
    assert_eq!(d.metadata.modality.as_deref(), Some("US"));
    assert_eq!(d.metadata.patient_id.as_deref(), Some("CREW-A"));
    let frames = d.pixels.unwrap().expect("pixel data");
    assert_eq!(
        (frames.rows, frames.columns, frames.bits_allocated),
        (4, 4, 8)
    );
    assert_eq!(frames.frames.len(), 1);
    assert_eq!(frames.frames[0].len(), 16);
    assert_eq!(frames.frames[0][15], 240);
    assert!(caelum_devices::files::dicom::read_file(b"DICM-not-really").is_err());
}
