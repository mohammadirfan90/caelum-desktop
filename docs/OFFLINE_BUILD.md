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
| WebView2 | Default mode downloads a bootstrapper at install time | `offlineInstaller`: the standalone WebView2 installer is embedded in the CAELUM installer. Flight builds may use `fixedRuntime` (pinned runtime). Both come from the cache |
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

## 3. Procedures (commands added in M0)
1. **Prepare** (online): fetch every item in §1 into the cache and write the manifest.
2. **Verify:** check every checksum against the manifest.
3. **Build offline:** with the network disabled, install UI dependencies from the cached store and
   run the release build against the vendored crates and cached bundler tools.
4. **Install offline:** the installer runs with no network. If WebView2 is missing (for example on
   Windows 10 LTSC 2019, which ships without it), the embedded offline installer installs it.
