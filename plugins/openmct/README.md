# plugins/openmct: CAELUM Open MCT plugin (F-GND-04)

Open MCT plugin package (TypeScript) for the flight-surgeon, BME and mission-control consoles.
It shows the CAELUM **OPS channel** (path B) in Open MCT:

- **Telemetry provider** for three object types:
  - `caelum.ops-alarm-stream`: every `ops_alarm` event; history from the gateway history endpoint.
  - `caelum.crew-status`: the highest active alarm of one crew member (one object per configured
    crew member).
  - `caelum.instance-status`: technical health of the CAELUM instance and its link to the onboard
    medical data system.

  Enumerated fields are numeric codes with Open MCT `enum` metadata, so tables show the text.
  Crew and instance status also implement the Open MCT staleness API (`supportsStaleness`,
  `isStale`, `subscribeToStaleness`), so native Open MCT views mark old or missing status as stale.
- **Crew status board** (`caelum.crew-board`): one row per crew member with priority as text and
  a glyph shape (no colour-only meaning), category, alarm state and packet time; **NO DATA** when
  nothing was received and **STALE** when the latest status is older than `staleAfterMs` (the old
  priority is then hidden, H-02). It also shows ground-link state, the rejected-message count and
  CAELUM instance health. Rows are keyboard focusable (Tab, Arrow keys, Home, End); changes are
  announced through `aria-live` regions (critical assertively).
- **Alert timeline** (`caelum.alert-timeline`): `ops_alarm` events grouped by alarm with their
  state transitions, loaded for the Open MCT time bounds and updated in realtime.

**No physiology (R-SEC-06, CLAUDE.md rule 6).** The OPS channel carries crew id, priority, alarm
category, alarm state, escalation stage, timestamps and technical health only: never a measured
value, parameter name, rule id or free text. The plugin validates every inbound message against
an exact field allow-list; a message with any unexpected field (for example `value`, `parameter`,
`ruleId` or `text`) is rejected whole. Rejection reasons never echo the unexpected field or value.

**No fabricated data (CLAUDE.md rule 2).** Nothing is defaulted, coerced or carried forward: a
missing status is NO DATA, an old one is STALE, a missing or invalid option throws. All text is
written with `textContent`.

## Installation

The plugin does not import `openmct`; it uses the instance passed to `install()`
(`peerDependencies: openmct >=4 <5`, not auto-installed).

```ts
import CaelumPlugin from '@caelum/openmct-plugin';

openmct.install(
  CaelumPlugin({
    baseUrl: 'https://ground-gateway.example/caelum', // http(s): GET {baseUrl}/history
    wsUrl: 'wss://ground-gateway.example/caelum/ops', // ws(s): realtime OPS messages
    staleAfterMs: 15_000, // status older than this (by packet time) is STALE
    crew: [{ id: '3f2b8c1e-7a4d-4e6f-9b0a-1c2d3e4f5a6b', label: 'Commander' }], // optional
  }),
);
```

| Option | Required | Meaning |
|---|---|---|
| `baseUrl` | yes | Ground gateway HTTP base URL (`http:` or `https:`). |
| `wsUrl` | yes | Ground gateway WebSocket URL (`ws:` or `wss:`). |
| `staleAfterMs` | yes | Positive staleness limit in milliseconds. |
| `crew` | no | Display labels for crew UUIDs (lowercase canonical, unique, non-empty label). Crew seen on the link without an entry are shown by UUID. |
| `transport` | no | Injected `WebSocket`, `fetch` and reconnect `backoff` (embedding and tests). Defaults to the browser globals; reconnect back-off is 1 s doubling to 30 s. |

There are no defaults for the required options: `CaelumPlugin()` throws
`CaelumPluginConfigError` before anything is registered. `install()` registers five object types,
the `caelum` object provider with a `CAELUM` root folder, the telemetry provider and the two views,
and opens the WebSocket.

## Data contract (OPS channel)

Every message is a JSON object with exactly these fields; anything else is rejected, counted and
dropped (never coerced). UUIDs are lowercase canonical and non-nil. Timestamps are ISO-8601 UTC
with a literal `Z` (`YYYY-MM-DDTHH:MM:SS[.f{1,9}]Z`). `apid` is 0..2047, `seq` 0..16383.

| `type` | Fields |
|---|---|
| `ops_alarm` | `crewId` (UUID), `priority` (`critical` \| `warning` \| `observe`), `category` (`CARDIO` \| `RESPIRATORY` \| `THERMAL` \| `ENVIRONMENT` \| `TECHNICAL`), `state` (`confirming` \| `active` \| `cleared_awaiting_ack` \| `acknowledged` \| `ended`), `escalationStage` (0..255), `createdAt`, `at`, `apid`, `seq` |
| `crew_status` | `crewId`, `highestPriority` (priority or `null`), `category` (or `null`), `state` (or `null`), `at`, `apid`, `seq`. The three are all `null` together when the crew member has no active alarm. |
| `instance_status` | `health` (`ok` \| `degraded` \| `fault`), `mdsLink` (`not_configured` \| `connected` \| `degraded` \| `unreachable`), `at`, `apid`, `seq` |

- **Realtime:** one WebSocket text frame per message on `wsUrl`. Binary frames are rejected.
- **History:** `GET {baseUrl}/history?type=ops_alarm&start=<ms>&end=<ms>` returns a JSON array of
  `ops_alarm` messages; each element is validated the same way. There is no history for crew or
  instance status; the provider returns the latest reading received in the session if it falls in
  the requested bounds, and nothing otherwise.
- **Display states:** no `crew_status` received = NO DATA; packet time older than `staleAfterMs`
  = STALE; all-null fields = NO ACTIVE ALARM (a received fact).
- **Alarm identity:** OPS carries no alarm id, so the timeline groups by
  (`crewId`, `createdAt`, `category`). Two alarms of the same category for the same crew member
  created in the same millisecond would be merged (documented limitation). Duplicate packets
  (same `apid`, `seq`, `at`) are dropped.

## Workspace

This package is **not** in the root `pnpm-workspace.yaml`; it has its own `pnpm-lock.yaml` and
`node_modules`. Whether to add it to the workspace (and to `pnpm -C tools/ci check-all`) is the
lead's decision.

## Development

```bash
cd plugins/openmct
pnpm install --frozen-lockfile
pnpm typecheck      # tsc, strict
pnpm test           # vitest run
pnpm build          # tsc -> dist/
```

Dev dependencies (exact pins): `typescript` 6.0.3, `vitest` 5.0.3, `vite` 8.3.4, `jsdom` 30.1.2.

Tests run in the Node environment. The DOM tests (crew board, alert timeline) create their own
jsdom window in a `beforeAll` hook (`test/dom.ts`) rather than using `environment: 'jsdom'`:
Vitest loads a test environment during worker start-up under a fixed 60 s timeout, and on slow or
virus-scanned Windows drives the first jsdom import can exceed it ("[vitest-pool-runner]: Timeout
waiting for worker to respond"). In a hook, the import is covered by the configurable
`hookTimeout` (180 s).

Test names cite F-GND-04, and R-SEC-06 where they check that physiology is rejected.

## Open items

- **Live Open MCT verification in CI.** The Open MCT API subset is declared locally
  (`src/openmct-types.ts`, Open MCT 4.x shapes, including the staleness API) and tested against a
  fake `openmct` object only. An end-to-end check that installs the plugin into a real Open MCT 4
  build is still to be added to CI.
- **Ground gateway endpoint.** The `history` and WebSocket endpoints are implemented by the lead
  in the CAELUM `ground` profile (`caelum-ground`); this package only consumes them.
- **Yamcs-backed alternative.** `docs/04_INTEGRATIONS.md` §3 allows the telemetry provider to
  read via Yamcs (XTCE-decoded CAELUM packets) instead of the CAELUM ground gateway. Not
  implemented; the crew board and alert timeline would need a Yamcs-backed client with the same
  validation.
- **Event-packet viewer.** `docs/04_INTEGRATIONS.md` §3 lists an event-packet viewer next to
  F-GND-04, but the feature lock (`docs/01_FEATURE_LOCK.md`) scopes F-GND-04 to telemetry
  provider, crew status board and alert timeline. Not built (scope is locked); the lead should
  reconcile the two documents.
