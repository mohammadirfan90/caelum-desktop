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
| WebView2 | Default mode downloads a bootstrapper at install time; Tauri 2.12's `offlineInstaller` mode contacts Microsoft on every build | `fixedRuntime` (CR-004): the pinned, Microsoft-signed WebView2 Fixed Version runtime (`webview2/` in the cache) is expanded into `src-tauri/webview2-fixed-runtime/` and shipped inside CAELUM; nothing is downloaded at build or install time |
| ONNX Runtime (M12) | `ort` downloads binaries at build time | `ort` with `load-dynamic`; a pinned, checksummed ONNX Runtime binary from the cache, shipped with the app |
| Advisory databases | cargo-deny, cargo-audit and pnpm audit download them | Online CI job on every run; a dated offline snapshot in the cache for air-gapped builds, whose reports state the snapshot date |
| Chromium for UI tests (M5) | Playwright downloads it on `playwright install` | Test only, never shipped: the build of the pinned Playwright is placed in `playwright/` in the cache; `check-all --offline` sets `PLAYWRIGHT_BROWSERS_PATH` to it |
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

1. **Prepare** (online, on a trusted machine; if Tauri's NSIS and WiX tools are not present yet,
   prepare runs one online `pnpm tauri build` so the Tauri CLI downloads them):
   `node tools/offline/prepare.ts --cache <dir>` (or set `CAELUM_TOOL_CACHE`). Vendors every crate,
   fetches the pnpm store, downloads the Rust and Node installers and pnpm (each checked against the
   digest its upstream publishes) and the WebView2 Fixed Version runtime (must carry a valid
   Microsoft Authenticode signature), copies the cargo check tools, Tauri's bundler tools and the
   advisory databases, and writes `ITEMS.json` (provenance) and `MANIFEST.sha256`.
   For an online release build, `node tools/offline/install-webview2.ts --cache <dir>` puts the
   runtime where `src-tauri/tauri.release.conf.json` expects it, then
   `pnpm tauri build --config src-tauri/tauri.release.conf.json`. A plain `pnpm tauri build` makes
   development installers that rely on the system WebView2 and are never shipped.
2. **Verify:** `node tools/offline/verify.ts --cache <dir>`. Fails on any changed, missing or
   unlisted file.
3. **Build offline:** with the network disabled, `node tools/offline/build-offline.ts --cache <dir>`.
   It verifies the cache first and refuses a damaged one, puts the bundler tools where the Tauri
   CLI looks, expands the WebView2 runtime (signature re-checked), installs npm packages from the
   cached store (`pnpm install --offline`), and builds
   against the vendored crates (`cargo --config` source replacement, `net.offline`). As a second
   line, it points every proxy-respecting client at a closed local port, so any network access
   the build still attempts fails loudly. It writes `target/offline/build-report.json` with the
   installer checksums.
4. **Install offline:** the installer runs with no network and installs per machine (Program
   Files). CAELUM uses its bundled WebView2 runtime, so it also runs where no system WebView2 is
   present (for example Windows 10 LTSC 2019).

Air-gapped checks: `node tools/ci/check-all.ts --offline` uses the advisory snapshot
(`cargo deny --offline check`, `cargo audit --no-fetch`); copy `advisory-db/cargo-deny` to
`~/.cargo/advisory-dbs` and `advisory-db/cargo-audit` to `~/.cargo/advisory-db` first. `pnpm audit`
has no offline mode; npm advisories are checked online in CI only. With `CAELUM_TOOL_CACHE` set, the
browser-mode UI tests (axe checks, keyboard walkthrough) run with the cached Chromium.

Linux and macOS bundler tools (AppImage, DMG) are not cached yet; Windows is the reference platform.
