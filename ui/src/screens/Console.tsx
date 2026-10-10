import { type SyntheticEvent, useId, useState } from "react";

import type { Api } from "../api";
import type { RoleName } from "../bindings";
import { type Notice, NoticeLine, useConfirmed } from "../components/Confirm";
import { useLoad } from "../components/useLoad";
import { Card, NoData, SectionHead } from "../components/Values";
import { strings } from "../strings";
import { utcDateTime } from "../time";
import { errorText } from "./Access";

const DAY_S = 86_400;
const unitless = (p: string) => strings.parameter[p] ?? p;

/** Downlinked summaries of one crew member (flight surgeon; MEDICAL channel). */
function Summaries({ api, crewId }: { readonly api: Api; readonly crewId: string }) {
  const rows = useLoad(() => api.consoleSummaries(crewId, DAY_S), crewId);
  return (
    <Card label={strings.consoleSummaries}>
      <SectionHead title={strings.consoleSummaries} />
      {rows.state === "error" ? <NoticeLine notice={{ kind: "error", text: rows.message }} /> : null}
      {rows.state === "ok" && rows.data.length === 0 ? (
        <div className="mt-3">
          <NoData status="noData" />
        </div>
      ) : null}
      {rows.state === "ok" && rows.data.length > 0 ? (
        <table className="mt-3 w-full text-left text-sm">
          <thead>
            <tr>
              {Object.values(strings.consoleSummaryColumns).map((h) => (
                <th key={h} scope="col">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {rows.data.map((r) => (
              <tr key={`${r.parameter}-${String(r.windowEndUtcMs)}`}>
                <td>{unitless(r.parameter)}</td>
                <td>{utcDateTime(r.windowEndUtcMs)}</td>
                <td className="tabular-nums">
                  {r.state !== null ? r.state : `${String(r.min ?? "")} / ${String(r.mean ?? "")} / ${String(r.max ?? "")}`}
                </td>
                <td>{String(r.count)}</td>
                <td>{strings.qualityName[r.quality] ?? r.quality}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
    </Card>
  );
}

/** Event packets received by CFDP (flight surgeon). */
function Files({ api }: { readonly api: Api }) {
  const files = useLoad(() => api.consoleFiles(), "files");
  const [open, setOpen] = useState<{ name: string; text: string } | null>(null);
  return (
    <Card label={strings.consoleFiles}>
      <SectionHead title={strings.consoleFiles} />
      {files.state === "ok" && files.data.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.consoleFilesNone}</p> : null}
      {files.state === "ok" && files.data.length > 0 ? (
        <ul className="mt-3 flex flex-col gap-2 text-sm">
          {files.data.map((f) => (
            <li key={f.id} className="flex flex-wrap items-center gap-3">
              <span>{`${f.name} · ${utcDateTime(f.receivedUtcMs)}`}</span>
              <button
                type="button"
                className="btn btn-soft"
                onClick={() => {
                  void api.consoleFile(f.id).then((r) => {
                    if (r.status === "ok") setOpen({ name: f.name, text: r.data });
                  });
                }}
              >
                {strings.consoleOpen(f.name)}
              </button>
            </li>
          ))}
        </ul>
      ) : null}
      {open === null ? null : (
        <section aria-label={open.name} className="mt-3">
          <pre className="whitespace-pre-wrap rounded-lg bg-inset p-3 text-xs">{open.text}</pre>
        </section>
      )}
    </Card>
  );
}

/** Private medical conference record (flight surgeon; signed once, then immutable). */
function Pmc({ api, crewId }: { readonly api: Api; readonly crewId: string }) {
  const notesId = useId();
  const [bump, setBump] = useState(0);
  const [notes, setNotes] = useState("");
  const [notice, setNotice] = useState<Notice>(null);
  const { run, prompt } = useConfirmed(api.authReauth);
  const list = useLoad(() => api.pmcList(crewId), `${crewId}:${String(bump)}`);
  const save = (e: SyntheticEvent) => {
    e.preventDefault();
    void api.pmcSave(null, crewId, Date.now(), notes).then((r) => {
      if (r.status === "error") setNotice({ kind: "error", text: errorText(r.error) });
      else {
        setNotes("");
        setNotice({ kind: "ok", text: strings.pmcSaved });
        setBump((b) => b + 1);
      }
    });
  };
  return (
    <Card label={strings.pmcTitle}>
      <SectionHead title={strings.pmcTitle} />
      {prompt}
      <form onSubmit={save} aria-label={strings.pmcNew} className="mt-3 flex flex-col gap-2">
        <label htmlFor={notesId} className="text-sm font-semibold">
          {strings.pmcNotes}
        </label>
        <textarea
          id={notesId}
          rows={4}
          className="rounded-lg border border-ink-3 bg-surface p-3"
          value={notes}
          onChange={(e) => {
            setNotes(e.target.value);
          }}
        />
        <button type="submit" className="btn btn-soft self-start">
          {strings.pmcSave}
        </button>
      </form>
      <NoticeLine notice={notice} />
      {list.state === "ok" ? (
        <ul className="mt-3 flex flex-col gap-3 text-sm">
          {list.data.map((r) => (
            <li key={r.id} className="flex flex-col gap-1">
              <span className="font-semibold">{`${utcDateTime(r.heldUtcMs)} · ${r.signed ? strings.pmcSigned : strings.pmcDraft}`}</span>
              <span className="whitespace-pre-wrap">{r.notes}</span>
              {r.signed ? null : (
                <button
                  type="button"
                  className="btn btn-ink self-start"
                  onClick={() => {
                    void run(
                      () => api.pmcSign(r.id),
                      () => {
                        setBump((b) => b + 1);
                      },
                      (err) => {
                        setNotice({ kind: "error", text: errorText(err) });
                      },
                    );
                  }}
                >
                  {strings.pmcSign}
                </button>
              )}
            </li>
          ))}
        </ul>
      ) : null}
    </Card>
  );
}

/** Downlinked OPS alarm records (flight surgeon, BME, MCC; no physiology). */
function OpsTimeline({ api }: { readonly api: Api }) {
  const rows = useLoad(() => api.consoleOps("ops_alarm", DAY_S), "ops");
  const parsed =
    rows.state === "ok"
      ? rows.data.flatMap((t) => {
          try {
            const v = JSON.parse(t) as { crewId: string; priority: string; category: string; state: string; at: string };
            return [v];
          } catch {
            return [];
          }
        })
      : [];
  return (
    <Card label={strings.consoleOps}>
      <SectionHead title={strings.consoleOps} />
      {rows.state === "ok" && parsed.length === 0 ? (
        <div className="mt-3">
          <NoData status="noData" />
        </div>
      ) : null}
      <ul className="mt-3 flex flex-col gap-1 text-sm">
        {parsed.map((a) => (
          <li key={`${a.crewId}-${a.at}-${a.state}`}>{strings.consoleOpsRow(a.at, a.priority, a.category, a.state)}</li>
        ))}
      </ul>
    </Card>
  );
}

/** Ground console profile (F-GND-08): what the downlink delivered. */
export function Console({ api, role, crewId }: { readonly api: Api; readonly role: RoleName; readonly crewId: string | null }) {
  const surgeon = role === "flight_surgeon";
  return (
    <div className="flex flex-col gap-6">
      <OpsTimeline api={api} />
      {surgeon ? <Files api={api} /> : null}
      {surgeon && crewId !== null ? <Summaries api={api} crewId={crewId} /> : null}
      {surgeon && crewId !== null ? <Pmc api={api} crewId={crewId} /> : null}
      {surgeon && crewId === null ? <p className="text-sm text-ink-3">{strings.chooseCrew}</p> : null}
    </div>
  );
}
