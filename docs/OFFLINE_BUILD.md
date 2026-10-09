# Offline build and install (R-OPS-03, F-SYS-06, CR-002 D4)

R-OPS-03: CAELUM installs, runs and updates with no internet access, and release builds are
reproducible offline from a prepared, checksummed tool cache. This document defines that cache.
The scripts that prepare and verify it are written in M0; their exact commands are added to §3 at
the M0 gate.

## 1. Everything that would otherwise need the network
| Item | Default behaviour | CAELUM handling |
|---|---|---|
| Rust toolchain | rustup downloads it | Version pinned in `rust-toolchain.toml`; installer archived in the cache |
| Rust crates | cargo downloads from crates.io | Vendored into the cache (`cargo vendor`); offline builds use source replacement and `CARGO_NET_OFFLINE=true` |
| npm packages | pnpm downloads them | pnpm store in the cache (`pnpm fetch`); offline installs use `--offline --frozen-lockfile` |
| NSIS, WiX | Tauri CLI downloads them on the first bundle | Vendored in the cache and placed where the Tauri CLI looks for them |
| WebView2 | Default mode downloads a bootstrapper at install time | `offlineInstaller`: the standalone WebView2 installer is embedded in the CAELUM installer. Flight builds may use `fixedRuntime` (pinned runtime). Both come from the cache. **Open (CR-004):** Tauri CLI 2.12 contacts Microsoft on every `offlineInstaller` build even when the installer is cached, so offline builds need the CR-004 decision |
| ONNX Runtime (M12) | `ort` downloads binaries at build time | `ort` with `load-dynamic`; a pinned, checksummed ONNX Runtime binary from the cache, shipped with the app |
| Advisory databases | cargo-deny, cargo-audit and pnpm audit download them | Online CI job on every run; a dated offline snapshot in the cache for air-gapped builds, whose reports state the snapshot date |
| Fonts | — | Outfit from `@fontsource/outfit`, bundled at build time; nothing fetched at runtime |
| LOINC table (M1) | Downloaded from loinc.org with an account | Placed by the product owner in `tools/offline/loinc/` (git-ignored); only the subset of codes used is committed, with the LOINC copyright notice and licence attribution |

## 2. Tool cache rules
- Lives outside the repository; its path is given by the environment variable `CAELUM_TOOL_CACHE`.
- `MANIFEST.sha256` lists every file with its SHA-256. A build from the cache refuses to start if
  any checksum fails.
- Every item records its source URL, version, fetch date and licence.
- Prepared online on a trusted machine and moved to air-gapped machines on media.

## 3. Procedures
Machine prerequisites, installed from their own offline media and not part of the cache: Visual
Studio Build Tools (C++ workload, Windows SDK) and, from M1, Strawberry Perl. Node.js and pnpm come
from the cache (`node/`, `pnpm/`); the Rust toolchain from `rust/` (if rustup is also installed, link
the installed toolchain as `1.99.0-x86_64-pc-windows-msvc` with `rustup toolchain link` so
`rust-toolchain.toml` resolves without the network).

1. **Prepare** (online, on a trusted machine, after one online `pnpm tauri build` so Tauri's bundler
   tools are present):
   `node tools/offline/prepare.ts --cache <dir>` (or set `CAELUM_TOOL_CACHE`). Vendors every crate,
   fetches the pnpm store, downloads the Rust and Node installers and pnpm (each checked against the
   digest its upstream publishes), copies the cargo check tools, Tauri's bundler tools and the
   advisory databases, and writes `ITEMS.json` (provenance) and `MANIFEST.sha256`.
2. **Verify:** `node tools/offline/verify.ts --cache <dir>`. Fails on any changed, missing or
   unlisted file.
3. **Build offline:** with the network disabled, `node tools/offline/build-offline.ts --cache <dir>`.
   It verifies the cache first and refuses a damaged one, puts the bundler tools where the Tauri
   CLI looks, installs npm packages from the cached store (`pnpm install --offline`), and builds
   against the vendored crates (`cargo --config` source replacement, `net.offline`). As a second
   line, it points every proxy-respecting client at a closed local port, so any network access
   the build still attempts fails loudly. It writes `target/offline/build-report.json` with the
   installer checksums. **Blocked until CR-004 is decided** (WebView2, §1).
4. **Install offline:** the installer runs with no network. If WebView2 is missing (for example on
   Windows 10 LTSC 2019, which ships without it), the embedded WebView2 installs it.

Air-gapped checks: `node tools/ci/check-all.ts --offline` uses the advisory snapshot
(`cargo deny --offline check`, `cargo audit --no-fetch`); copy `advisory-db/cargo-deny` to
`~/.cargo/advisory-dbs` and `advisory-db/cargo-audit` to `~/.cargo/advisory-db` first. `pnpm audit`
has no offline mode; npm advisories are checked online in CI only.

Linux and macOS bundler tools (AppImage, DMG) are not cached yet; Windows is the reference platform.
