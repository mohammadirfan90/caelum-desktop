# M8 clinical files: station integration

How the `caelum-devices::files` parsers (CR-014) are wired into the station, the IPC and the
Medical screen. Requirement IDs: F-ACQ-06, F-ACQ-07, F-ACQ-08, F-CLN-03, F-CLN-04, SR-016,
SR-017.

## Import (`crates/caelum-core/src/station/files.rs`)
| Kind | What happens | Kept when |
|---|---|---|
| `dicom` | `dicom::read_file`; identity (SOP class/instance, study, modality, date, manufacturer, model, transfer syntax) and pixel summary (rows, columns, frames, or why the pixels cannot be shown) go into the attachment metadata | the object opens |
| `ecg` | detected by content: DICOM waveform (`DICM` preamble), HL7 aECG (`AnnotatedECG`), EDF/EDF+ (version `0`), otherwise SCP-ECG; format, lead labels, rate and duration go into the metadata | the recording opens and every lead has samples and a positive rate |
| `file` | kept as it is (size only) | always (size limits of the attachment store) |
| FHIR (`fhir_import`) | `fhir::import`; each observation goes through `caelum_ingest::ingest` (unit, plausibility, time-in-future, SR-017), is stored with classification `imported` (quality `degraded`: FHIR states none) and evaluated by the alarm engine; resources that are not imported are listed with the reason; the original JSON is kept as a `file` attachment | the crew member is active on the roster in force |

Files that do not open are refused with the parser's reason and nothing is stored. Every import
is audited (`attachment.import`, `fhir.import`). In test-equipment builds, FHIR resources tagged
`HTEST` are kept as `simulated` (never exported), as for device readings.

## Review
- `ecg_view` (F-CLN-03): the stored recording, decoded again from the encrypted store (no second
  copy). The viewer shows one lead at a time in a 10 s window, min/max per column so no peak is
  lost, with two keyboard-operable time calipers that give the interval, the rate (60 000 / ms)
  and the amplitude at each caliper in the file's own unit.
- `dicom_view` (F-CLN-04): one frame rendered to 8-bit display samples in the core. Grey images:
  stored bits masked, sign extended, rescale slope/intercept applied, then the linear VOI window
  of DICOM PS3.3 C.11.2.1.2 (the requested window, else the file's first preset, else the frame's
  own value range, so nothing is assumed); MONOCHROME1 inverted. 8-bit RGB (interleaved or
  planar) shown as stored. Other colour models are refused with a reason. Pixel spacing is shown
  when stated; without it no distances are offered.

## Roles
`fhir_import`, `ecg_view`, `dicom_view`: CMO and Flight Surgeon (`docs/ROLE_MATRIX.md` §3).

## Open
- Folder watch and the DICOM C-STORE SCP (the SCP needs its own network CR, CR-014 §2).
- ECG review: events from EDF+ annotations and aECG, export of a reviewed segment.
- Attaching an image or recording to an encounter or an event packet.
- Retention policies (only the Mission class exists).
