import { strings } from "./strings";

/**
 * Display times (docs/07 §5: times in UTC). The core converts TAI to UTC with its leap-second
 * table and sends Unix milliseconds; `null` means the time is not known and is shown as such.
 */
export function utcTime(ms: number | null): string {
  if (ms === null) return strings.timeUnknown;
  return strings.utc(new Date(ms).toISOString().slice(11, 19));
}

export function utcDateTime(ms: number | null): string {
  if (ms === null) return strings.timeUnknown;
  const iso = new Date(ms).toISOString();
  return strings.utc(`${iso.slice(0, 10)} ${iso.slice(11, 19)}`);
}
