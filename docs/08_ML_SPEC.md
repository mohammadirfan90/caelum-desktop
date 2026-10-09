# 08. Onboard models

## 1. Rules
- Models are **advisory**. They never raise high-priority alarms on their own and never suppress
  rules (SR-002). Output appears as notices with explanations and a validation status.
- Runtime: ONNX via `ort`, CPU only, fixed thread count, per-inference time limit.
- Each model package: ONNX file, SHA-256, model card (data, metrics, limits), feature spec,
  signature from the model approver. Unverified packages are rejected.
- Validation status shown in the UI: `VALIDATED`, `INVESTIGATIONAL`, `DISABLED`.

## 2. Models
| ID | Purpose | Inputs | Training data (public) | Gate to ship |
|---|---|---|---|---|
| M1 | Signal quality | ECG, PPG, resp windows + IMU | MIT-BIH + Noise Stress Test, PPG-DaLiA | AUROC ≥ 0.90 on held-out subjects |
| M2 | Rhythm (AF and others) | Single-lead ECG 30 s | Icentia11k (pretrain), PhysioNet/CinC 2017, MIT-BIH | AF F1 ≥ 0.80 subject-wise |
| M3 | Respiration events | RIP, ECG-derived respiration | BIDMC, CapnoBase, Apnea-ECG | rate MAE < 2 /min |
| M4 | Sleep staging | HR, HRV, resp, temp, activity (no posture) | DREAMT, Sleep-Accel, MESA | sleep/wake ≥ 85 % |
| M5 | Stress index | HRV, EDA, resp, temp | WESAD | F1 ≥ 0.80 leave-one-subject-out |
| M6 | BP trend | PAT, PPG morphology | PulseDB, VitalDB | trend direction ≥ 85 % after calibration |
| M7 | Fitness / recovery | HR vs workload | Treadmill maximal exercise tests (PhysioNet) | VO₂max error < 10 % |
| M8 | Short-term deterioration risk | 30–60 min of vitals | MIMIC-IV, VitalDB | ≥ 15 min lead time at agreed false-alert rate |
| M9 | Personal-baseline anomaly | Daily feature vector | Pre-flight baseline per astronaut | detect injected drifts in ≤ 3 days, < 1 false notice / week |

## 3. Pipeline (outside the app, in `ml/`)
Data cards → subject-wise splits → training → calibration → ONNX export → test against the
frozen evaluation set → model card → approver signature → package. The app only loads packages.
