import { useEffect, useState } from "react";

import type { CommandError, RulesetInfo } from "./bindings";
import { strings } from "./strings";

/**
 * SR-009 banner (docs/07 §3): a persistent "DEVELOPMENT RULESET" notice when the ruleset in force is
 * development-signed, and an alert when the bundled ruleset was rejected (fallback) or no verified
 * ruleset exists (physiological alarms not running). Text and glyph, never colour alone. If the
 * status cannot be read, that is shown too: an unknown ruleset state is never presented as fine.
 */

export type RulesetResult = { status: "ok"; data: RulesetInfo } | { status: "error"; error: CommandError };

type Load =
  | { readonly state: "loading" }
  | { readonly state: "ready"; readonly info: RulesetInfo }
  | { readonly state: "failed"; readonly reason: string };

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return strings.unknownError;
}

export function RulesetBanner({ load }: { readonly load: () => Promise<RulesetResult> }) {
  const [status, setStatus] = useState<Load>({ state: "loading" });

  useEffect(() => {
    let current = true;
    load().then(
      (r) => {
        if (!current) return;
        setStatus(r.status === "ok" ? { state: "ready", info: r.data } : { state: "failed", reason: r.error.message });
      },
      (error: unknown) => {
        if (current) setStatus({ state: "failed", reason: describe(error) });
      },
    );
    return () => {
      current = false;
    };
  }, [load]);

  if (status.state === "loading") return null;
  if (status.state === "failed") {
    return <Alert title={strings.rulesetUnknown} detail={status.reason} />;
  }
  const info = status.info;
  if (info.state === "safeState") {
    return <Alert title={strings.rulesetSafeState} detail={info.reason ?? ""} />;
  }
  const development = info.development ? (
    <aside
      aria-label={strings.rulesetLabel}
      className="w-full border-b border-warning/40 bg-warning/10 px-4 sm:px-6 py-2 backdrop-blur-md"
    >
      <div className="flex items-center justify-between gap-4">
        <p className="flex items-center gap-2.5 text-xs sm:text-sm font-semibold tracking-wide text-warning-ink">
          <TechnicalGlyph />
          <span>{strings.developmentRuleset(info.version ?? "")}</span>
        </p>
      </div>
    </aside>
  ) : null;
  if (info.state === "fallback") {
    return (
      <>
        {development}
        <Alert title={strings.rulesetFallback(info.version ?? "")} detail={info.reason ?? ""} />
      </>
    );
  }
  return development;
}

function Alert({ title, detail }: { readonly title: string; readonly detail: string }) {
  return (
    <div
      role="alert"
      aria-label={strings.rulesetLabel}
      className="w-full border-b border-alarm-high/40 bg-alarm-high/15 px-4 sm:px-6 py-2.5 backdrop-blur-md"
    >
      <div className="flex items-start gap-3">
        <TechnicalGlyph />
        <div>
          <p className="text-sm font-semibold text-ink">{title}</p>
          {detail.length > 0 ? <p className="mt-0.5 text-xs text-ink-3">{detail}</p> : null}
        </div>
      </div>
    </div>
  );
}

/** Technical-problem glyph: square with "!" (docs/06_SAFETY_AND_ALARMS.md §2); never colour alone. */
export function TechnicalGlyph() {
  return (
    <span
      aria-hidden="true"
      className="inline-flex size-6 shrink-0 items-center justify-center rounded border border-warning bg-warning/20 text-xs font-bold text-warning-ink shadow-xs"
    >
      !
    </span>
  );
}
