# CAELUM Desktop: build pack for Claude Code

This pack locks the features and gives Claude Code everything it needs to build CAELUM as a
Tauri 2 + Rust desktop application.

## How to use it
1. Create an empty repository, copy this pack into it, and keep `CLAUDE.md` at the root.
   Claude Code reads `CLAUDE.md` automatically at the start of every session.
2. Unzip `reference/caelum-next-prototype-v3.1.zip` into `reference/prototype/` (git-ignored).
   It is a **reference only** for design tokens, components, rules and tests. It is never shipped.
3. Open Claude Code in the repo, paste the first prompt from `START_PROMPT.md`, then work through `docs/10_BUILD_PLAN.md` one milestone at a time.
   Each milestone has a ready-to-paste prompt and acceptance criteria. Do not start the next
   milestone until the current one passes its gate.
4. Any change to `docs/01_FEATURE_LOCK.md` goes through the change-control rule in that file.

## Files
| File | Purpose |
|---|---|
| `START_PROMPT.md` | **The prompts to paste into Claude Code** (first session, each milestone, recovery) |
| `CLAUDE.md` | Permanent instructions for Claude Code: rules, stack, commands, definition of done |
| `docs/00_ANALYSIS.md` | Analysis of the prototype and of what "fully working" requires |
| `docs/01_FEATURE_LOCK.md` | **Locked feature set** for v1.0 (rev. 3), with IDs, priorities and out-of-scope list |
| `docs/02_NASA_REQUIREMENTS.md` | NASA and international standards, with the requirement-to-feature trace |
| `docs/03_ARCHITECTURE.md` | Process model, Rust crates, data flow, security, fail-over |
| `docs/04_INTEGRATIONS.md` | Every external interface: devices, vehicle, mission control, medical records |
| `docs/05_DATA_MODEL.md` | Database schema, parameter catalog, provenance, retention, encryption |
| `docs/06_SAFETY_AND_ALARMS.md` | Alarm system, rule engine, escalation, hazard analysis, safety requirements |
| `docs/07_UI_SPEC.md` | Design system (carried over from the prototype), screens, roles, accessibility |
| `docs/08_ML_SPEC.md` | Onboard models, runtime, governance and validation gates |
| `docs/09_VV_PLAN.md` | Verification and validation: tests, test equipment, performance budgets |
| `docs/10_BUILD_PLAN.md` | 14 milestones with Claude Code prompts and acceptance gates |
| `docs/11_ICD_TEMPLATE.md` | Interface Control Document template for mission control and vehicle interfaces |
| `docs/icd/ICD-MDS.md` | Draft ICD for the onboard medical data system (MDA-class) connection |
| `docs/icd/ICD-VEH.md` | Draft ICD for the vehicle (cFS) connection, including how status telemetry enters cFS |
| `docs/OFFLINE_BUILD.md` | Offline build and install: the checksummed tool cache and procedures |
| `docs/write-exceptions.md` | Reviewed register of every permitted device write (empty until M2) |
| `docs/third-party-risk.md` | SOUP register: third-party dependencies with known risks and mitigations |
| `docs/change-requests/CR-001.md` | Approved change adding the medical data system integration (feature lock rev. 2) |
| `docs/change-requests/CR-002.md` | Decisions from the first-session review (feature lock rev. 3) |
| `docs/change-requests/CR-003.md` | Medical lead, spot-check evaluation, measured fever, licence and dependency policy |
| `docs/change-requests/CR-004.md` | WebView2 Fixed Version runtime for offline-reproducible Windows builds |
| `docs/ROLE_MATRIX.md` | Who may see and do what, per role (M1) |
| `docs/change-requests/CR-005.md` | M0 offline evidence by network trap; physical network-off run in M13 |
| `LICENSE`, `NOTICE` | Apache-2.0 licence and attribution notice |
