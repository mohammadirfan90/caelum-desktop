// Verify the offline tool cache against its manifest — docs/OFFLINE_BUILD.md §3.2.
//   node tools/offline/verify.ts --cache <dir>      (or set CAELUM_TOOL_CACHE)
import { cacheDir, readItems, verifyCache } from "../ci/src/offline.ts";

const cache = cacheDir(process.argv.slice(2));
const problems = await verifyCache(cache);
if (problems.length > 0) {
  console.error(`verify: tool cache ${cache} is NOT intact:`);
  for (const p of problems) console.error(`  ${p}`);
  process.exit(1);
}
const items = readItems(cache);
console.log(`verify: ${cache} intact (${String(items.length)} items)`);
for (const item of items) console.log(`  ${item.name} ${item.version} — fetched ${item.fetchedAt.slice(0, 10)}`);
