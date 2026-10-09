# Prompts for Claude Code

## 1. First session (paste this once, in the empty repo with this pack at its root)

```
You are the lead engineer building CAELUM Desktop, an onboard astronaut health monitoring system:
a Tauri 2 desktop app with a Rust core. It ingests wearable and medical-device data plus read-only
vehicle data from cFS, runs deterministic safety alarms and escalation for the crew, writes the
health record to the onboard medical data system (FHIR R4), and sends operational status and
alarms to mission control as CCSDS telemetry (Yamcs / Open MCT).

Before writing any code:
1. Read CLAUDE.md and every file in docs/ in numeric order (00 to 11), plus
   docs/change-requests/CR-001.md and docs/icd/ICD-MDS.md.
2. Unzip reference/caelum-next-prototype-v3.1.zip into reference/prototype/ and study it as a
   reference only (design tokens, components, alert rules and their tests). Never ship its code.
3. Reply with: (a) a one-page summary of the system as you understand it, (b) any contradictions or
   gaps you found in the docs, (c) the tools I must install on this machine (Rust toolchain, Node,
   pnpm, Tauri prerequisites for this OS, SQLCipher build needs), and (d) your step-by-step plan
   for milestone M0 from docs/10_BUILD_PLAN.md.
Then wait for my approval.

Rules for the whole project:
- Build only what docs/01_FEATURE_LOCK.md lists. If something seems missing, draft a change request
  in docs/change-requests/ and ask me; never add unlisted features.
- No fabricated data, no demo mode, no placeholder values. Test equipment only behind the
  `test-equipment` Cargo feature.
- No command path to vehicle systems or therapeutic devices.
- Work in small steps. After each step: run tests, clippy and UI checks, show me the result, and
  commit with the requirement IDs it implements.
- At the end of a milestone, demonstrate every item in its gate, update docs/trace.csv, and stop.
```

## 2. Every following milestone (replace Mx)

```
Read CLAUDE.md, docs/01_FEATURE_LOCK.md, docs/06_SAFETY_AND_ALARMS.md and milestone Mx in
docs/10_BUILD_PLAN.md (plus any docs it references). Show me a step-by-step plan for Mx with the
requirement IDs each step covers. After I approve, implement step by step with tests, keep CI green,
and stop at the Mx gate with evidence for every gate item.
```

## 3. When something breaks or drifts

```
Stop. Re-read CLAUDE.md and the current milestone. List what you changed since the last green
commit, which rule or requirement it may violate, and propose the smallest fix. Do not continue
until tests, clippy and the gate checks for completed milestones are green again.
```
