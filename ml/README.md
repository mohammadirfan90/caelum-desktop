# CAELUM ML: Model Training Pipeline

This directory contains the machine learning pipeline for CAELUM Desktop models (docs/08 §3, docs/10 M12, CR-018).

## Models Overview
| ID | Model Name | Primary Gate | Reported Parameter | Output Notice |
|---|---|---|---|---|
| **M1** | Signal Quality Index | AUROC ≥ 0.90 on held-out subjects | `sqi_ecg`, `sqi_ppg` | `< 0.50`: Degraded signal quality |
| **M2** | Cardiac Rhythm (AF) | AF F1 ≥ 0.80 subject-wise | `af_probability` | `> 0.80`: Irregular rhythm suspected (feeds `af.observe`) |
| **M4** | Sleep Staging | Sleep/wake accuracy ≥ 85% | `sleep_stage` | `> 0.70`: Crew member asleep |
| **M9** | Personal Baseline Anomaly | Drift detected in ≤ 3 days | - | `> 0.65`: Multi-day physiological drift detected |
| **M3** | Respiration Events | Rate MAE < 2 /min | `respiration_rate` | - |
| **M5** | Stress Index | F1 ≥ 0.80 LOSO | - | `> 0.75`: Elevated sympathetic stress index |
| **M6** | Blood Pressure Trend | Trend direction ≥ 85% | `bp_trend_systolic` | - |

## Training on Google Colab
1. Open Google Colab: https://colab.research.google.com
2. Copy and execute the cells provided in `docs/caelum_model_training_plan.md` or upload this repository.
3. When Cell 8 completes, it downloads `caelum_models.zip`.
4. Extract `caelum_models.zip` into the `models/` directory in this workspace:
   ```text
   models/
     M1/
       model.onnx
       manifest.toml
       manifest.sig.toml
     M2/...
     M4/...
     M9/...
   ```

## Signing & Trust Policy
In development builds (`debug_assertions`), CAELUM trusts public keys listed in `crates/caelum-alarms/keys/dev.pub`.
The training pipeline outputs the signer's public key hex and key ID:
```text
<public_key_hex>  # key id <key_id>
```
To trust the Colab-trained models locally, ensure this entry exists in `crates/caelum-alarms/keys/dev.pub`.
Release builds require flight keys and medical approver signatures.
