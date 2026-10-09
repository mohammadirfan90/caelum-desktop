# tools/

Command-line tools and build scripts (docs/03_ARCHITECTURE.md §2). Filled in by milestone:

| Tool | Milestone |
|---|---|
| `ci/check-release-features.ts`: release dependency graph contains no test equipment (CLAUDE.md rule 3) | M0 |
| `ci/check-writes.ts`: no unregistered device writes, no writes in `caelum-vehicle` (CLAUDE.md rule 5) | M0 |
| `ci/check-npm-licenses.ts`: npm packages shipped in the UI use only allowed licences (CR-003 §4) | M0 |
| `ci/sbom.ts`: CycloneDX SBOMs for the app (Rust, via cargo-cyclonedx) and the UI (npm) into `target/sbom/` | M0 |
| `ci/trace.ts`: runs all test suites, writes `docs/trace.csv`, enforces complete milestones (`docs/09_VV_PLAN.md` §3) | M0 |
| `offline/`: tool-cache prepare / verify / offline build (`docs/OFFLINE_BUILD.md`) | M0 |
| `offline/loinc/`: LOINC table placed by the product owner (git-ignored) | M1 |
| Ruleset signer | M3 |
| Config signer, XTCE export CLI, data export CLI | M10 |

`tools/ci` is a pnpm workspace package run directly by Node 24 (TypeScript type stripping):
`pnpm -C tools/ci check`, `pnpm -C tools/ci test`, `pnpm -C tools/ci typecheck`.
