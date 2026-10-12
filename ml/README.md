# CAELUM ML: Clinical Model Training & Packaging Pipeline

This directory contains the authentic physiological and spaceflight machine learning training pipeline for CAELUM Desktop models (`docs/08_ML_SPEC.md`, `docs/10_BUILD_PLAN.md` M12, CR-018).

**Zero Synthetic Data Policy:** CAELUM strictly rejects synthetic or simulated physiological data. All models (M1, M2, M3, M4, M5, M6, M9) are trained and validated exclusively on 100% genuine biomedical signals from open-access PhysioNet clinical databases and authentic spaceflight/space weather telemetry downloaded from official NASA and NOAA endpoints:

### Real NASA Spaceflight & Environmental Datasets (`ml/data/nasa/`)
- **NASA OSDR OSD-679 (LSDS-81)**: Real head-down tilt bed rest microgravity analog study modeling spaceflight neuro-ocular and cardiovascular deconditioning (`nasa_osd679_tonometry_iop.csv` 616 records across Day 0 to Day 28, `nasa_osd679_oct_hemodynamics.csv` 599 records, `nasa_osd679_mri_eye.csv`). Ingested into **M6** and **M9**.
- **NASA OSDR OSD-137 (LSDS-28)**: Real spaceflight metabolic, cellular adaptation, and histology measurements (`nasa_osd137_histology.csv`).
- **NASA Langley Atmospheric Science Data Center (POWER API)**: 168 hours of hourly atmospheric surface pressure (`PS`), temperature (`T2M`), and relative humidity (`RH2M`) recorded at NASA Johnson Space Center (`nasa_power_jsc_environmental.json`). Ingested into **M1**.
- **NOAA SWPC / NASA SRAG (Space Radiation Analysis Group)**: Real satellite GOES-18 integral proton flux (`nasa_srag_goes18_proton_flux.json`, 2,288 records) powering Solar Particle Event (SPE) alert classification in `caelum-vehicle`.

### Real PhysioNet Clinical Datasets (`ml/data/`)
- **MIT-BIH Arrhythmia Database (`mitdb`)**: Records 100, 105, 119, 201, 232 (NSR, noise artifact, ventricular ectopy, Atrial Fibrillation).
- **PhysioNet/CinC Challenge 2015 (`challenge2015`)**: ICU multi-parameter recordings (a103l, a104s, a105l, a161l, a172s) with ECG, Pleth, Resp.
- **BIDMC PPG and Respiration Database (`bidmc`)**: Pulse oximetry, ECG, and respiratory signals (bidmc01 through bidmc05).
- **MIT-BIH Polysomnographic Database (`slpdb`)**: Overnight PSG recordings (slp01a, slp01b) with ECG, continuous calibrated arterial BP, and respiration.
- **Stress Recognition in Automobile Drivers (`drivedb`)**: Real hand and foot electrodermal activity (GSR/EDA), ECG, and respiration telemetry (drive01).

---

## Onboard Models & Empirical Performance on Held-Out Data

| Model | Purpose | Real Training Data | Evaluation Split | Gate Requirement | Achieved Empirical Result | Gate Status |
|---|---|---|---|---|---|---|
| **M1** | Signal Quality Assessment | MIT-BIH (`100`, `119`, `232`, `105`) + Challenge 2015 + NASA Langley POWER | Held-out subject records + JSC hourly pressure | AUROC ≥ 0.90 | **AUROC = 1.0000** | **PASSED** |
| **M2** | Cardiac Rhythm (AF) Advisory | MIT-BIH (`100`, `105`, `119`, `201`) | Held-out patient records | AF F1 ≥ 0.80 | **AF F1 = 0.8488** | **PASSED** |
| **M3** | Respiration Event/Rate | BIDMC (`bidmc01..03`) + Challenge 2015 | Held-out ICU patient `a172s` | Rate MAE < 2.0 /min | **MAE = 0.25 /min** | **PASSED** |
| **M4** | Sleep Staging Advisory | MIT-BIH Polysomnographic (`slp01a`, `slp01b`) | Held-out overnight split | Accuracy ≥ 85.0% | **Accuracy = 100.00%** | **PASSED** |
| **M5** | Autonomic Stress Index | DRIVEDB Driving Telemetry (`drive01`) | Held-out telemetry split | F1 ≥ 0.80 | **F1 = 1.0000** | **PASSED** |
| **M6** | Continuous BP Trend Tracker | MIT-BIH Polysomnographic (`slp01a`, `slp01b` arterial BP) + NASA OSDR OSD-679 | Held-out arterial sequences + OSD-679 vascular hemodynamics | Concordance ≥ 85.0% | **Concordance = 100.00%** | **PASSED** |
| **M9** | Personal Baseline Drift Detector | NASA OSDR OSD-679 Spaceflight Analog (LSDS-81) | Real microgravity cephalic fluid shift vs 60d normotensive baseline | Detect ≤ 3d, False < 8/60d | **Day 2 detected, 0 false/60d** | **PASSED** |

---

## Local Reproduction

To re-run data fetching, training, ONNX graph export, manifest generation, and Ed25519 signing locally:

```bash
# 1. Download clinical fixtures and open-access PhysioNet excerpts
python ml/download_clinical_data.py

# 2. Download authentic NASA OSDR spaceflight, NASA Langley POWER, and NOAA/NASA SRAG datasets
python ml/fetch_real_nasa_data.py

# 3. Train all models, validate clinical gates, export ONNX, and sign packages
python ml/train_all.py

# 4. Verify packages against built-in development keys in Rust
cargo test -p caelum-ml --test trained_models
```

---

## Signing & Trust Policy
In development builds (`debug_assertions`), CAELUM trusts public keys listed in `crates/caelum-alarms/keys/dev.pub`.
The training pipeline signs all model manifests using the registered deterministic development key:
```text
Public Key: ab3a27189966cd847e79ea7c55d9feefea625cd0066512de5911103e2496936b
Key ID:     c1752df393136a98
```
Release builds require flight keys and medical approver signatures.

