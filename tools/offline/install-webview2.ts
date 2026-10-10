// Puts the pinned WebView2 Fixed Version runtime from the tool cache into
// src-tauri/webview2-fixed-runtime, where tauri.conf.json expects it (CR-004). Needed before any
// Windows `pnpm tauri build`; build-offline.ts does this itself.
//   node tools/offline/install-webview2.ts --cache <dir>      (or set CAELUM_TOOL_CACHE)
import { join } from "node:path";

import { cacheDir } from "../ci/src/offline.ts";
import { installRuntime, RUNTIME_DIR, WEBVIEW2 } from "../ci/src/webview2.ts";

const cache = cacheDir(process.argv.slice(2));
const sig = installRuntime(join(cache, "webview2", WEBVIEW2.cab), false);
console.log(`install-webview2: WebView2 ${WEBVIEW2.version} (${sig.status}, ${sig.subject.split(",")[0] ?? ""}) -> ${RUNTIME_DIR}`);
