import { type SyntheticEvent, useEffect, useId, useState } from "react";

import type { CommandError, CrewLanView, RemoteAlarm } from "../bindings";
import { level } from "../components/AlarmBar";
import { NoticeLine } from "../components/Confirm";
import { LevelGlyph } from "../components/LevelGlyph";
import { NoData } from "../components/Values";
import { strings } from "../strings";
import { errorText } from "./Access";

type Res<T> = { status: "ok"; data: T } | { status: "error"; error: CommandError };

/** What the crew-station screen needs (the generated bindings; injectable for tests). */
export interface CrewStationApi {
  readonly crewLanStatus: () => Promise<Res<CrewLanView>>;
  readonly crewLanSignIn: (username: string, secret: string) => Promise<Res<string>>;
  readonly crewLanSignOut: () => Promise<Res<null>>;
}

function AlarmRow({ a }: { readonly a: RemoteAlarm }) {
  const l = level(a.priority);
  return (
    <li className="card flex items-center gap-4 p-5 text-2xl font-semibold">
      <LevelGlyph level={l} size={40} />
      {`${strings.level[l]}: ${a.title ?? a.category} · ${strings.crewStationState(a.state, a.stage)}`}
    </li>
  );
}

/**
 * A crew station on the crew LAN (F-SYS-05, F-SYS-13; CR-017): a secondary display (users sign
 * in on the primary) or a kiosk (crew-wide emergency view, no sign-in, SR-014). It shows only
 * what the primary streams; without a link it shows NO DATA, never the last known state.
 */
export function CrewStation({ api, onChange }: { readonly api: CrewStationApi; readonly onChange: (h: () => void) => () => void }) {
  const userId = useId();
  const secretId = useId();
  const [view, setView] = useState<CrewLanView | null>(null);
  const [username, setUsername] = useState("");
  const [secret, setSecret] = useState("");
  const [notice, setNotice] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  useEffect(() => {
    const load = () => {
      void api.crewLanStatus().then((r) => {
        if (r.status === "ok") setView(r.data);
      });
    };
    load();
    return onChange(load);
  }, [api, onChange]);
  const signIn = (e: SyntheticEvent) => {
    e.preventDefault();
    void api.crewLanSignIn(username, secret).then((r) => {
      setSecret("");
      setNotice(r.status === "ok" ? null : { kind: "error", text: errorText(r.error) });
    });
  };
  if (view === null) return null;
  const title = view.kiosk ? strings.kioskTitle : strings.crewStationTitle;
  return (
    <main id="content" tabIndex={-1} className="flex w-full flex-col gap-6 p-8">
      <h1 className="text-4xl font-semibold">{title}</h1>
      <p className="text-lg" role="status">
        {view.connected ? strings.crewStationConnected : (view.problem ?? strings.crewStationNoLink)}
      </p>
      {!view.kiosk && view.signedIn === null ? (
        <form onSubmit={signIn} aria-label={strings.crewStationSignIn} className="flex flex-wrap items-end gap-4">
          <div className="flex flex-col gap-1">
            <label htmlFor={userId} className="text-sm font-semibold">
              {strings.username}
            </label>
            <input
              id={userId}
              className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
              autoComplete="username"
              value={username}
              onChange={(e) => {
                setUsername(e.target.value);
              }}
            />
          </div>
          <div className="flex flex-col gap-1">
            <label htmlFor={secretId} className="text-sm font-semibold">
              {strings.secret}
            </label>
            <input
              id={secretId}
              type="password"
              className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
              autoComplete="current-password"
              value={secret}
              onChange={(e) => {
                setSecret(e.target.value);
              }}
            />
          </div>
          <button type="submit" className="btn btn-ink">
            {strings.crewStationSignIn}
          </button>
        </form>
      ) : null}
      {!view.kiosk && view.signedIn !== null ? (
        <div className="flex items-center gap-4">
          <p>{strings.crewStationSignedIn(view.signedIn)}</p>
          <button
            type="button"
            className="btn btn-soft"
            onClick={() => {
              void api.crewLanSignOut();
            }}
          >
            {strings.signOut}
          </button>
        </div>
      ) : null}
      <NoticeLine notice={notice} />
      {!view.connected ? (
        <NoData status="noData" />
      ) : view.alarms.length === 0 ? (
        <p className="flex items-center gap-3 text-2xl">
          <LevelGlyph level="normal" size={36} />
          {strings.emergencyNoAlarms}
        </p>
      ) : (
        <ul className="flex flex-col gap-4">
          {view.alarms.map((a) => (
            <AlarmRow key={a.alarmId} a={a} />
          ))}
        </ul>
      )}
    </main>
  );
}
