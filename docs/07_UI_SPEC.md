# 07. UI specification (design pattern carried over from the prototype)

## 1. Design tokens (copy `frontend/src/styles/tokens.css` from the prototype verbatim)
Light: canvas `#D6E8E2`, surface `#FFFFFF`, inset `#E6F3EF`, border `#C2DFD6`, ink `#141414`,
ink-3 `#58736B`, nav `#000000`, signal `#006D58`, signal-soft `#E6F5F1`, warning `#D4A017`.
Dark: canvas `#000000`, surface `#141414`, raised `#2B2B2B`, ink `#FFFFFF`, signal `#2DD4BF`,
warning `#F5B800`. Card radius 12 px; pill buttons (`.btn`, min height 48 px).
Fonts: Outfit (bundled locally via @fontsource; never fetched at runtime), tabular numerals for all
values (`.dot-num`). Alarm colours (critical red, warning amber) are added as tokens
`--alarm-high`, `--alarm-medium`, used only together with shapes.

## 2. Primitives (port from prototype, then extend)
`LevelGlyph` (circle/diamond/triangle/octagon/technical square), `DotNum`, `Stat`, `Card`,
`SectionHead`, `EmptyState`, `NoData` (NO DATA / STALE / SIGNAL UNUSABLE), `EcgLine` (now drawn
from real waveform chunks), `HeartRateBars`, `Charts` (baseline band), `Sheet`, `SlideToAct`
(for irreversible actions), `WhyDrawer` (alarm explanation), new `AlarmBar`, `EscalationTracker`,
`ProtocolChecklist`, `DeviceTile`, `WaveformViewer` (calipers), `DicomViewer`.

## 3. Desktop layout
```
┌ Alarm bar (always visible; highest active alarm, crew-wide emergencies, ack/silence) ──────┐
├ Nav rail ┬ Crew strip: selected crew member, live vitals with status, device health ──────┤
│ Home     │                                                                                  │
│ Body     │  Content (multi-pane on wide screens: list + detail)                            │
│ Alerts   │                                                                                  │
│ Checks   │                                                                                  │
│ Env      │                                                                                  │
│ Medical* │                                                                                  │
│ Devices  │                                                                                  │
│ Ground*  │                                                                                  │
│ Settings │                                                                                  │
└──────────┴ Status footer: link state, ground link (AOS/LOS), instance role (primary/standby) ┘
```
`*` visible only to CMO, flight surgeon, BME, administrator. Supports multiple windows (e.g.
waveform viewer on a second monitor) and a kiosk mode for a wall-mounted crew display.

## 4. Screens (v1.0 only, per feature lock)
| Screen | Roles | Content |
|---|---|---|
| Home | all crew | Own status level, live vitals, active alarms, next check, environment summary |
| Body | crew, medical | Heart (live ECG, HR, HRV), breathing, temperature, sleep, trends vs baseline |
| Alerts | all | Active and recent alarms, why, protocol, escalation tracker, acknowledge/silence |
| Checks | crew | Scheduled checks, PVT test, guided device sessions, daily check-in |
| Environment | all | Cabin atmosphere, radiation, correlation with physiology |
| Medical | CMO, surgeon | Crew overview, encounters, ECG review, DICOM viewer, protocol library |
| Devices | crew, CMO, admin | Pair, assign, battery, signal quality, calibration, clock offset |
| Ground | surgeon, BME, MCC, admin | Downlink queue, link state, uplink verification, reports |
| Audit | CMO, surgeon, admin | Signed audit log with verification status |
| Settings | all | Theme, text size, glove mode, sound, accessibility; admin config |
| Emergency (break-glass) | anyone | Crew-wide emergency, location, protocol checklist |

## 5. Interaction and accessibility rules
- Every value shows status: LIVE (no badge), STALE (age), NO DATA, SIGNAL UNUSABLE.
- Glove mode: targets ≥ 56 px, spacing ≥ 12 px, no hover-only actions, no drag-only actions.
- Text scaling to 200 % without loss; high-contrast theme; full keyboard operation; screen reader labels.
- No colour-only meaning; shapes and text always present.
- Irreversible or safety actions use `SlideToAct` or confirm dialogs with the consequence stated.
- Motion respects reduced-motion; no animation conveys information alone.
- Plain language; units always shown; times in UTC and MET.
