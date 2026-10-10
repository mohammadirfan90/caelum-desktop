import { useEffect, useState } from "react";

import { AlarmSounder, type AudioLike } from "./alarmSound";
import { type AlarmBusEvent, commands, events, type AppInfo } from "./bindings";
import { RulesetBanner, type RulesetResult, TechnicalGlyph } from "./RulesetBanner";
import { strings } from "./strings";

/**
 * M0 window: product name and the version reported by the Rust core (F-SYS-06).
 * The version is only ever shown as reported over IPC; if the call fails the window says so and
 * shows no version at all (CLAUDE.md rule 2: no fabricated values).
 */

type Load =
  | { readonly state: "loading" }
  | { readonly state: "ready"; readonly info: AppInfo }
  | { readonly state: "failed"; readonly reason: string };

export interface AppProps {
  /** Source of the application identity; the generated IPC command by default. */
  readonly loadAppInfo?: () => Promise<AppInfo>;
  /** Source of the alarm ruleset status (SR-009 banner); the generated IPC command by default. */
  readonly loadRuleset?: () => Promise<RulesetResult>;
  /** Subscription to the alarm bus; the generated typed event by default. Returns an unsubscribe. */
  readonly subscribeAlarms?: (handler: (e: AlarmBusEvent) => void) => Promise<() => void>;
  /** Audio factory for alarm sounds. */
  readonly makeAudio?: (src: string) => AudioLike;
}

const newAudio = (src: string): AudioLike => new Audio(src);

const listenAlarms = (handler: (e: AlarmBusEvent) => void) =>
  events.alarmBusEvent.listen((e) => {
    handler(e.payload);
  });

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return "unknown error";
}

export function App({
  loadAppInfo = commands.appInfo,
  loadRuleset = commands.rulesetStatus,
  subscribeAlarms = listenAlarms,
  makeAudio = newAudio,
}: AppProps) {
  const [load, setLoad] = useState<Load>({ state: "loading" });

  // Alarm sound on this station (SR-015): independent of sign-in and of any wristband.
  useEffect(() => {
    const sounder = new AlarmSounder(makeAudio);
    let unsubscribe: (() => void) | undefined;
    let current = true;
    subscribeAlarms((e) => {
      sounder.handle(e);
    }).then(
      (stop) => {
        if (current) unsubscribe = stop;
        else stop();
      },
      () => undefined,
    );
    return () => {
      current = false;
      unsubscribe?.();
    };
  }, [subscribeAlarms, makeAudio]);

  useEffect(() => {
    let current = true;
    loadAppInfo().then(
      (info) => {
        if (current) setLoad({ state: "ready", info });
      },
      (error: unknown) => {
        if (current) setLoad({ state: "failed", reason: describe(error) });
      },
    );
    return () => {
      current = false;
    };
  }, [loadAppInfo]);

  return (
    <main className="flex min-h-full flex-col items-center justify-center bg-canvas p-6 text-ink">
      <RulesetBanner load={loadRuleset} />
      <section aria-labelledby="app-title" className="card w-full max-w-[34rem] p-8">
        <p className="label">{strings.tagline}</p>
        <h1 id="app-title" className="mt-2 text-[2.75rem] leading-tight font-semibold tracking-[-0.03em]">
          {strings.productName}
        </h1>
        {load.state === "failed" ? (
          <div role="alert" className="mt-6 flex items-start gap-3">
            <TechnicalGlyph />
            <div>
              <p className="text-base font-semibold">{strings.versionUnavailable}</p>
              <p className="mt-1 text-sm text-ink-3">{strings.coreNoResponse(load.reason)}</p>
            </div>
          </div>
        ) : (
          <p role="status" aria-live="polite" className="mt-6 text-base text-ink-3 tabular-nums">
            {load.state === "ready" ? strings.version(load.info.version) : strings.loading}
          </p>
        )}
      </section>
    </main>
  );
}
