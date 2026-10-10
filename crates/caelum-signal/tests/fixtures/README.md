# caelum-signal reference-record fixtures (M7 gate, CR-013 §12)

Small, unmodified byte excerpts of open-licence PhysioNet databases, used only by the
caelum-signal integration tests (test fixtures, never product data). Headers (`.hea`) and
annotation files (`.atr`) are the original files; `.dat` files are byte ranges of the original
signal files, cut at frame boundaries.

## MIT-BIH Arrhythmia Database v1.0.0 (`mitdb/`)
Licence: Open Data Commons Attribution License v1.0 (ODC-By 1.0),
<https://physionet.org/content/mitdb/1.0.0/>.

| File | Excerpt (format 212, 2 channels, 360 Hz) | Use |
|---|---|---|
| `100.dat` | bytes 0–194 399 = samples 0–64 799 (first 180 s) | clean QRS accuracy |
| `105.dat` | bytes 0–194 399 = first 180 s | noisy QRS accuracy, SQI |
| `119.dat` | bytes 0–194 399 = first 180 s | ventricular bigeminy (ectopic beats) |
| `232.dat` | bytes 1 101 600–1 166 399 = samples 367 200–388 799 (1020–1080 s) | sinus pauses up to 5.9 s |

Attribution:
- Moody GB, Mark RG. The impact of the MIT-BIH Arrhythmia Database. IEEE Eng in Med and Biol
  20(3):45–50 (May–June 2001).
- Goldberger AL, Amaral LAN, Glass L, Hausdorff JM, Ivanov PCh, Mark RG, Mietus JE, Moody GB,
  Peng C-K, Stanley HE. PhysioBank, PhysioToolkit, and PhysioNet: Components of a new research
  resource for complex physiologic signals. Circulation 101(23):e215–e220 (2000).

## PhysioNet/CinC Challenge 2015 training set v1.0.0 (`challenge2015/`)
Licence: Open Data Commons Attribution License v1.0 (ODC-By 1.0),
<https://physionet.org/content/challenge-2015/1.0.0/>.

Each record's alarm was triggered at 300 s; the event starts between 290 and 300 s. The `.dat`
files are the bytes of the original `.mat` files after the 24-byte header, from sample 62 500
(250 s) to the end of the record (format 16, little endian, interleaved, 250 Hz; gains in the
`.hea` files; channel 0 is ECG lead II).

| Record | Alarm | Label | Use |
|---|---|---|---|
| `a161l`, `a172s` | Asystole | true alarm (pause ≥ 6 s in lead II) | `asystole.critical` must fire |
| `a103l`, `a104s`, `a105l` | Asystole | false alarm | must not fire (noise or artefact) |

Attribution:
- Clifford GD, Silva I, Moody B, Li Q, Kella D, Shahin A, Kooistra T, Perry D, Mark RG. The
  PhysioNet/Computing in Cardiology Challenge 2015: Reducing false arrhythmia alarms in the ICU.
  Computing in Cardiology 2015;42:273–276.
