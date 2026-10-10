import { useEffect, useState } from "react";

import type { Res } from "../api";
import { errorText } from "../screens/Access";

/** State of a command whose result a screen shows. */
export type Loaded<T> =
  | { readonly state: "loading" }
  | { readonly state: "ok"; readonly data: T }
  | { readonly state: "error"; readonly message: string };

/**
 * Runs `load` now and whenever `key` changes (a selection, or a change event counter); a failure
 * is shown as its reason, never replaced by made-up data (CLAUDE.md rule 2).
 */
export function useLoad<T>(load: (() => Promise<Res<T>>) | null, key: string): Loaded<T> {
  const [result, setResult] = useState<Loaded<T>>({ state: "loading" });
  useEffect(() => {
    if (load === null) return;
    let current = true;
    // A failure before the call even starts (e.g. the command is not available) is shown like
    // any other failure, never thrown out of the effect.
    let pending: Promise<Res<T>>;
    try {
      pending = load();
    } catch (e: unknown) {
      pending = Promise.reject(e instanceof Error ? e : new Error(String(e)));
    }
    pending.then(
      (r) => {
        if (!current) return;
        setResult(r.status === "ok" ? { state: "ok", data: r.data } : { state: "error", message: errorText(r.error) });
      },
      (e: unknown) => {
        if (current) setResult({ state: "error", message: e instanceof Error ? e.message : String(e) });
      },
    );
    return () => {
      current = false;
    };
    // `key` decides when to reload; `load` is a new closure on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);
  return result;
}
