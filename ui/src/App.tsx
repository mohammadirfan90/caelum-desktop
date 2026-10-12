import { useCallback, useEffect, useReducer, useRef, useState } from "react";

import { AlarmSounder, type AudioLike } from "./alarmSound";
import type { Api, DataEvents, Res } from "./api";
import { type AlarmMap, ranked, reduce } from "./alarmStore";
import { type AlarmBusEvent, type AppInfo, commands, events, type SessionInfo } from "./bindings";
import { AlarmBar } from "./components/AlarmBar";
import { RulesetBanner, TechnicalGlyph } from "./RulesetBanner";
import { errorText, Locked, type Result, SignIn } from "./screens/Access";
import { EmergencyView, Shell } from "./screens/Shell";
import { KioskBoard } from "./screens/Windows";
import { applyPrefs, loadPrefs, type Prefs, savePrefs } from "./settings";
import { strings } from "./strings";

/**
 * The CAELUM window (docs/07 §3): ruleset banner and alarm bar always on top, whatever the session
 * state (alarms never depend on sign-in); below, first-administrator enrolment, sign-in, the
 * emergency view, the lock screen or the signed-in shell. Every value shown comes from the core
 * over IPC; failures are shown, never replaced by made-up values (CLAUDE.md rule 2).
 */

export type { Api } from "./api";

export interface AppProps {
  readonly api?: Partial<Api>;
  /** Subscription to the alarm bus; the generated typed event by default. Returns an unsubscribe. */
  readonly subscribeAlarms?: (handler: (e: AlarmBusEvent) => void) => Promise<() => void>;
  /** Audio factory for alarm sounds. */
  readonly makeAudio?: (src: string) => AudioLike;
  /** Value-free change events (new readings, device changes); the typed events by default. */
  readonly dataEvents?: DataEvents;
  /** How often the session state is read (auto-lock detection), ms. */
  readonly pollMs?: number;
}

const newAudio = (src: string): AudioLike => new Audio(src);

/** The value-free change events (CR-012 §4), from the generated typed events. */
const tauriDataEvents: DataEvents = {
  onObservations: (handler) =>
    events.observationsChanged.listen((e) => {
      handler(e.payload.crewId);
    }),
  onDevices: (handler) =>
    events.devicesChanged.listen(() => {
      handler();
    }),
};

const listenAlarms = (handler: (e: AlarmBusEvent) => void) =>
  events.alarmBusEvent.listen((e) => {
    handler(e.payload);
  });

/** SR-008: the longest silence; the core clamps per priority. */
const SILENCE_REQUEST_S = 600;

function describe(error: unknown): string {
  if (error instanceof Error) return error.message;
  if (typeof error === "string") return error;
  return strings.unknownError;
}

type Phase =
  | { readonly kind: "loading" }
  | { readonly kind: "setup" }
  | { readonly kind: "signedOut" }
  | { readonly kind: "emergency"; readonly audited: boolean; readonly back: "signedOut" | "locked" }
  | { readonly kind: "locked"; readonly session: SessionInfo }
  | { readonly kind: "active"; readonly session: SessionInfo };

type Load = { readonly state: "loading" } | { readonly state: "ready"; readonly info: AppInfo } | { readonly state: "failed"; readonly reason: string };

function done(r: Res<unknown>): Result {
  return r.status === "ok" ? { status: "ok" } : { status: "error", error: r.error };
}

export function App({ api: overrides = {}, subscribeAlarms = listenAlarms, makeAudio = newAudio, dataEvents = tauriDataEvents, pollMs = 10_000 }: AppProps) {
  const [api] = useState<Api>(() => ({ ...commands, ...overrides }));
  const [load, setLoad] = useState<Load>({ state: "loading" });
  const [phase, setPhase] = useState<Phase>({ kind: "loading" });
  const [alarms, dispatch] = useReducer(reduce, new Map() as AlarmMap);
  const [actionError, setActionError] = useState<string | null>(null);
  const [prefs, setPrefs] = useState<Prefs>(loadPrefs);

  useEffect(() => {
    applyPrefs(prefs);
  }, [prefs]);

  // Keyboard users keep their place (docs/07 §5): when the window changes state (signed in,
  // locked, emergency view opened or closed) the focused control is gone, so focus moves to the
  // new content instead of falling back to the start of the page. Not on start-up, so the first
  // Tab still reaches the skip link.
  const shownPhase = useRef<Phase["kind"]>("loading");
  useEffect(() => {
    const before = shownPhase.current;
    shownPhase.current = phase.kind;
    if (before !== "loading" && before !== phase.kind) document.getElementById("content")?.focus();
  }, [phase.kind]);

  // Alarm sound and alarm state on this station (SR-015): independent of sign-in.
  useEffect(() => {
    const sounder = new AlarmSounder(makeAudio);
    let unsubscribe: (() => void) | undefined;
    let current = true;
    subscribeAlarms((e) => {
      sounder.handle(e);
      dispatch(e);
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
    api.appInfo().then(
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
  }, [api]);

  /** Reads the session state (not activity: polling cannot keep a session unlocked). */
  const refresh = useCallback(async () => {
    const setup = await api.setupStatus().catch(() => null);
    if (setup?.status === "ok" && setup.data) {
      setPhase({ kind: "setup" });
      return;
    }
    const me = await api.authMe().catch(() => null);
    setPhase((p) => {
      if (p.kind === "emergency") return p;
      if (me?.status !== "ok") return { kind: "signedOut" };
      return me.data.locked ? { kind: "locked", session: me.data } : { kind: "active", session: me.data };
    });
  }, [api]);

  useEffect(() => {
    const first = setTimeout(() => {
      void refresh();
    }, 0);
    const timer = setInterval(() => {
      void refresh();
    }, pollMs);
    return () => {
      clearTimeout(first);
      clearInterval(timer);
    };
  }, [refresh, pollMs]);

  const after = async (r: Res<unknown>): Promise<Result> => {
    if (r.status === "ok") await refresh();
    return done(r);
  };

  const act = (r: Promise<Res<unknown>>) => {
    setActionError(null);
    void r.then((x) => {
      if (x.status === "error") setActionError(errorText(x.error));
    });
  };

  const openEmergency = (back: "signedOut" | "locked") => {
    void api.breakGlassOpen().then((r) => {
      setPhase({ kind: "emergency", audited: r.status === "ok" && r.data.audited, back });
    });
  };

  const onPrefs = (p: Prefs) => {
    setPrefs(p);
    savePrefs(p);
  };

  const list = ranked(alarms);
  let body;
  switch (phase.kind) {
    case "loading":
      body = null;
      break;
    case "setup":
      body = (
        <SignIn
          needsSetup
          onSignIn={async (u, s) => after(await api.authLogin(u, s))}
          onCreateAdministrator={async (u, s) => after(await api.setupCreateAdministrator(u, s))}
          onEmergency={() => {
            openEmergency("signedOut");
          }}
        />
      );
      break;
    case "signedOut":
      body = (
        <SignIn
          needsSetup={false}
          onSignIn={async (u, s) => after(await api.authLogin(u, s))}
          onCreateAdministrator={async (u, s) => after(await api.setupCreateAdministrator(u, s))}
          onEmergency={() => {
            openEmergency("signedOut");
          }}
        />
      );
      break;
    case "emergency":
      body = (
        <EmergencyView
          alarms={list}
          audited={phase.audited}
          onBack={() => {
            setPhase({ kind: "loading" });
            void refresh();
          }}
        />
      );
      break;
    case "locked":
      body = (
        <Locked
          name={phase.session.username}
          onUnlock={async (s) => after(await api.authReauth(s))}
          onSignOut={() => {
            void api.authLogout().then(() => refresh());
          }}
          onEmergency={() => {
            openEmergency("locked");
          }}
        />
      );
      break;
    case "active":
      body = null;
      break;
  }

  return (
    <div className="hud-canvas flex min-h-full flex-col text-ink">
      <a href="#content" className="skip-link">
        {strings.skipToContent}
      </a>
      <RulesetBanner load={api.rulesetStatus} />
      <AlarmBar
        alarms={list}
        prefs={prefs}
        onPrefs={onPrefs}
        onAcknowledge={(id) => {
          act(api.alarmAcknowledge(id));
        }}
        onSilence={(id) => {
          act(api.alarmSilence(id, SILENCE_REQUEST_S));
        }}
      />
      {actionError === null ? null : (
        <div role="alert" className="border-b border-alarm-high/30 bg-alarm-high/10 px-4 py-2.5 text-sm font-semibold text-ink">
          <div className="mx-auto flex max-w-7xl items-center gap-2">
            <TechnicalGlyph />
            <span>{actionError}</span>
          </div>
        </div>
      )}
      {load.state === "ready" && load.info.kiosk ? (
        <main id="content" tabIndex={-1} className="flex flex-1 flex-col">
          <KioskBoard alarms={list} />
        </main>
      ) : phase.kind === "active" ? (
        <Shell
          api={api}
          alarms={list}
          events={dataEvents}
          session={phase.session}
          prefs={prefs}
          onPrefs={onPrefs}
          onSignOut={() => {
            void api.authLogout().then(() => refresh());
          }}
        />
      ) : (
        <main id="content" tabIndex={-1} className="relative flex flex-1 flex-col items-center justify-center p-6 md:p-10">
          <div className="relative z-10 flex w-full max-w-[32rem] flex-col items-stretch gap-6">
            <section aria-labelledby="app-title" className="card group relative overflow-hidden p-8 backdrop-blur-xl">
              <div className="pointer-events-none absolute top-0 left-0 right-0 h-[2px] bg-gradient-to-r from-transparent via-signal to-transparent opacity-80" />
              <div className="flex items-start justify-between gap-4">
                <div className="flex flex-col">
                  <div className="flex items-center gap-2">
                    <span className="relative flex size-2">
                      <span className="absolute inline-flex h-full w-full animate-ping rounded-full bg-signal opacity-75" />
                      <span className="relative inline-flex size-2 rounded-full bg-signal" />
                    </span>
                    <p className="label font-mono tracking-widest text-[0.7rem] text-signal font-medium uppercase">
                      {strings.tagline}
                    </p>
                  </div>
                  <h1 id="app-title" className="mt-2 text-4xl sm:text-5xl font-bold tracking-tight text-ink">
                    {strings.productName}
                  </h1>
                </div>

                <div className="hidden sm:flex size-14 items-center justify-center rounded-2xl border border-signal/25 bg-signal-soft/40 text-signal shadow-inner">
                  <svg className="size-8" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.75">
                    <circle cx="12" cy="12" r="9" strokeOpacity="0.4" strokeDasharray="3 3" />
                    <circle cx="12" cy="12" r="5" />
                    <line x1="12" y1="2" x2="12" y2="7" />
                    <line x1="12" y1="17" x2="12" y2="22" />
                    <line x1="2" y1="12" x2="7" y2="12" />
                    <line x1="17" y1="12" x2="22" y2="12" />
                    <circle cx="12" cy="12" r="1.5" fill="currentColor" />
                  </svg>
                </div>
              </div>

              {load.state === "failed" ? (
                <div role="alert" className="mt-6 flex items-start gap-3 rounded-lg border border-alarm-high/30 bg-alarm-high/10 p-4">
                  <TechnicalGlyph />
                  <div>
                    <p className="text-base font-semibold text-ink">{strings.versionUnavailable}</p>
                    <p className="mt-1 text-sm text-ink-3">{strings.coreNoResponse(load.reason)}</p>
                  </div>
                </div>
              ) : (
                <div className="mt-6 flex items-center justify-between border-t border-border/60 pt-4 text-xs font-mono">
                  <p role="status" aria-live="polite" className="text-ink-3 tracking-wide">
                    {load.state === "ready" ? strings.version(load.info.version) : strings.loading}
                  </p>
                  <span aria-hidden="true" className="flex items-center gap-1.5 text-signal">
                    <span className="size-2 rounded-full bg-signal animate-pulse" />
                  </span>
                </div>
              )}
            </section>
            {body}
          </div>
        </main>
      )}
    </div>
  );
}
