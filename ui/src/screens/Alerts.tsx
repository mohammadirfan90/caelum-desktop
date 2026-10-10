import { useId, useState } from "react";

import type { Api, Res } from "../api";
import type { AlarmEntry, AlarmInfo, CrewMember, RoleName } from "../bindings";
import { level } from "../components/AlarmBar";
import { NoticeLine, type Notice } from "../components/Confirm";
import { LevelGlyph } from "../components/LevelGlyph";
import { useLoad } from "../components/useLoad";
import { Card, SectionHead } from "../components/Values";
import { strings } from "../strings";
import { utcDateTime, utcTime } from "../time";
import { errorText } from "./Access";

/** SR-008: the longest silence asked for; the core clamps per priority. */
const SILENCE_REQUEST_S = 600;

/** Title shown for an alarm: the ruleset title, the technical kind, or (OPS view) the category. */
export function alarmTitle(a: AlarmEntry): string {
  if (a.title !== null) return a.title;
  if (a.kind === "technical") {
    const kind = a.ruleId.split(".")[1] ?? "";
    return strings.technicalTitle[kind] ?? strings.technicalOther;
  }
  return strings.alertCategory[a.category] ?? a.category;
}

function stateText(a: AlarmEntry): string {
  if (a.resolvedAtUtcMs !== null) return strings.alertEnded;
  return strings.alertState[a.state] ?? a.state;
}

/** The engine's `why` as label/value pairs (technical record of the rule evaluation). */
function Why({ json }: { readonly json: string }) {
  let entries: [string, string][] = [];
  try {
    const parsed: unknown = JSON.parse(json);
    if (parsed !== null && typeof parsed === "object") {
      entries = Object.entries(parsed as Record<string, unknown>).map(([k, v]) => [k, typeof v === "string" ? v : JSON.stringify(v)]);
    }
  } catch {
    entries = [];
  }
  return (
    <dl className="grid grid-cols-[minmax(8rem,auto)_1fr] gap-x-4 gap-y-1 text-sm">
      {entries.map(([k, v]) => (
        <div key={k} className="contents">
          <dt className="font-semibold">{k}</dt>
          <dd className="break-all">{v}</dd>
        </div>
      ))}
    </dl>
  );
}

function Detail({
  api,
  info,
  onClose,
  onChanged,
}: {
  readonly api: Api;
  readonly info: AlarmInfo;
  readonly onClose: () => void;
  readonly onChanged: () => void;
}) {
  const [notice, setNotice] = useState<Notice>(null);
  const a = info.alarm;
  const act = (p: Promise<Res<unknown>>) => {
    void p.then((r) => {
      if (r.status === "error") setNotice({ kind: "error", text: errorText(r.error) });
      else onChanged();
    });
  };
  const open = a.resolvedAtUtcMs === null && a.acknowledgedAtUtcMs === null;
  return (
    <section aria-labelledby="detail-title" className="card flex flex-col gap-4 p-5">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h3 id="detail-title" className="flex items-center gap-3 text-xl font-semibold">
          <LevelGlyph level={level(a.priority)} />
          {`${strings.level[level(a.priority)]}: ${alarmTitle(a)}`}
        </h3>
        <button type="button" className="btn btn-soft" onClick={onClose}>
          {strings.detailClose}
        </button>
      </div>
      <p className="text-sm">{`${stateText(a)} · ${utcDateTime(a.createdAtUtcMs)}`}</p>
      {info.action === null ? null : (
        <div>
          <h4 className="font-semibold">{strings.detailAction}</h4>
          <p>{info.action}</p>
        </div>
      )}
      {info.triggerValue === null ? null : <p className="text-sm">{`${strings.detailTrigger}: ${String(info.triggerValue)}`}</p>}
      <div>
        <h4 className="font-semibold">{strings.detailWhy}</h4>
        <Why json={info.why} />
      </div>
      <p className="text-sm text-ink-3">{`${strings.detailRuleset}: ${info.rulesetVersion}`}</p>
      <div>
        <h4 className="font-semibold">{strings.detailTimeline}</h4>
        <ol className="flex flex-col gap-1 text-sm">
          {info.timeline.map((s) => (
            <li key={s.stage}>{`${(strings.audience as Record<string, string | undefined>)[s.audience] ?? s.audience} · ${utcTime(s.atUtcMs)}`}</li>
          ))}
        </ol>
      </div>
      <NoticeLine notice={notice} />
      {open ? (
        <div className="flex flex-wrap gap-3">
          <button
            type="button"
            className="btn btn-ink"
            onClick={() => {
              act(api.alarmAcknowledge(a.id));
            }}
          >
            {strings.acknowledge}
          </button>
          {a.priority === "observe" ? null : (
            <button
              type="button"
              className="btn btn-soft"
              onClick={() => {
                act(api.alarmSilence(a.id, SILENCE_REQUEST_S));
              }}
            >
              {strings.silence}
            </button>
          )}
          <button
            type="button"
            className="btn btn-soft"
            onClick={() => {
              act(api.alarmEscalate(a.id));
            }}
          >
            {strings.escalate}
          </button>
        </div>
      ) : null}
    </section>
  );
}

function Report({ api }: { readonly api: Api }) {
  const windowId = useId();
  const [windowS, setWindowS] = useState(86400);
  const report = useLoad(() => api.alarmReport(windowS), String(windowS));
  return (
    <Card label={strings.reportTitle}>
      <SectionHead title={strings.reportTitle} />
      <div className="mt-3 flex flex-col gap-1">
        <label htmlFor={windowId} className="text-sm font-semibold">
          {strings.reportWindow}
        </label>
        <select
          id={windowId}
          className="min-h-12 max-w-[16rem] rounded-lg border border-ink-3 bg-surface px-3"
          value={windowS}
          onChange={(e) => {
            setWindowS(Number(e.target.value));
          }}
        >
          {[86400, 604800, 2592000].map((w) => (
            <option key={w} value={w}>
              {strings.reportWindows[w]}
            </option>
          ))}
        </select>
      </div>
      {report.state === "error" ? <NoticeLine notice={{ kind: "error", text: report.message }} /> : null}
      {report.state === "ok" && report.data.rules.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.reportEmpty}</p> : null}
      {report.state === "ok" && report.data.rules.length > 0 ? (
        <table className="mt-4 w-full text-left text-sm">
          <thead>
            <tr>
              {Object.values(strings.reportColumns).map((h) => (
                <th key={h} scope="col">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {report.data.rules.map((r) => (
              <tr key={`${r.ruleId}/${r.priority}`}>
                <td>{r.ruleId}</td>
                <td>{strings.level[level(r.priority)]}</td>
                <td>{r.count}</td>
                <td>{r.medianAckS === null ? strings.noData.noData : strings.seconds(r.medianAckS)}</td>
                <td>{r.ackedEarly}</td>
                <td>{r.silenced}</td>
                <td>{r.selfResolved}</td>
                <td>{r.repeatBursts}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
    </Card>
  );
}

/** Alerts (docs/07 §4, F-ALM-05, F-ALM-09): active and recent alarms, why, escalation, actions. */
export function Alerts({
  api,
  userRole: role,
  crew,
  version,
}: {
  readonly api: Api;
  readonly userRole: RoleName;
  readonly crew: readonly CrewMember[];
  /** Reload key: changes when the alarm bus reports something. */
  readonly version: number;
}) {
  const [selected, setSelected] = useState<string | null>(null);
  const [bump, setBump] = useState(0);
  const key = `${String(version)}:${String(bump)}`;
  const list = useLoad(() => api.alarmList(), key);
  const detail = useLoad(selected === null ? null : () => api.alarmDetail(selected), `${selected ?? ""}:${key}`);
  const crewName = (id: string | null) => crew.find((c) => c.id === id)?.name ?? strings.noData.noData;
  return (
    <div className="flex flex-col gap-6">
      {selected !== null && detail.state === "ok" ? (
        <Detail
          api={api}
          info={detail.data}
          onClose={() => {
            setSelected(null);
          }}
          onChanged={() => {
            setBump((b) => b + 1);
          }}
        />
      ) : null}
      {selected !== null && detail.state === "error" ? <NoticeLine notice={{ kind: "error", text: detail.message }} /> : null}
      <Card label={strings.alertsTitle}>
        <SectionHead title={strings.alertsTitle} />
        {list.state === "error" ? <NoticeLine notice={{ kind: "error", text: list.message }} /> : null}
        {list.state === "ok" && list.data.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.alertsEmpty}</p> : null}
        {list.state === "ok" && list.data.length > 0 ? (
          <table className="mt-4 w-full text-left text-sm">
            <thead>
              <tr>
                {Object.values(strings.alertColumns).map((h) => (
                  <th key={h} scope="col">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {list.data.map((a) => (
                <tr key={a.id}>
                  <td>
                    <span className="flex items-center gap-2">
                      <LevelGlyph level={level(a.priority)} />
                      {strings.level[level(a.priority)]}
                    </span>
                  </td>
                  <td>{alarmTitle(a)}</td>
                  <td>{crewName(a.crewMemberId)}</td>
                  <td>{stateText(a)}</td>
                  <td>{utcTime(a.createdAtUtcMs)}</td>
                  <td>{a.escalationStage}</td>
                  <td>
                    {a.opsOnly ? (
                      strings.alertOpsOnly
                    ) : (
                      <button
                        type="button"
                        className="btn btn-soft"
                        aria-label={strings.alertShowLabel(alarmTitle(a))}
                        onClick={() => {
                          setSelected(a.id);
                        }}
                      >
                        {strings.alertShow}
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : null}
      </Card>
      {role === "cmo" || role === "flight_surgeon" || role === "bme" ? <Report api={api} /> : null}
    </div>
  );
}
