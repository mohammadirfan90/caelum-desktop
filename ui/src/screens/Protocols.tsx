import { useState } from "react";

import type { Api } from "../api";
import type { ChecklistRun, ProtocolItem, Step } from "../bindings";
import { type Notice, NoticeLine } from "../components/Confirm";
import { useLoad } from "../components/useLoad";
import { Card, SectionHead } from "../components/Values";
import { strings } from "../strings";
import { utcDateTime } from "../time";
import { errorText } from "./Access";

function Run({
  api,
  run,
  protocol,
  canRun,
  onChanged,
}: {
  readonly api: Api;
  readonly run: ChecklistRun;
  readonly protocol: ProtocolItem | undefined;
  readonly canRun: boolean;
  readonly onChanged: (n: Notice) => void;
}) {
  const mark = (i: number, m: Step, complete = false) => {
    void api.checklistUpdate(run.id, i, m, complete).then((r) => {
      onChanged(r.status === "error" ? { kind: "error", text: errorText(r.error) } : null);
    });
  };
  const done = run.completedAtUtcMs !== null;
  const title = protocol?.title ?? run.protocolId;
  return (
    <li className="flex flex-col gap-2 border-t border-border pt-3">
      <p className="font-semibold">{`${title} · ${utcDateTime(run.startedAtUtcMs)}`}</p>
      {done ? <p className="text-sm">{strings.runCompleted(utcDateTime(run.completedAtUtcMs))}</p> : null}
      <ol className="flex flex-col gap-2">
        {run.steps.map((s, i) => {
          const text = protocol?.steps[i] ?? String(i + 1);
          return (
            <li key={text} className="flex flex-wrap items-center gap-3">
              <span>{`${String(i + 1)}. ${text}`}</span>
              <span className="text-sm text-ink-3">{strings.stepState[s] ?? s}</span>
              {!done && canRun && s === "open" ? (
                <>
                  <button
                    type="button"
                    className="btn btn-soft"
                    aria-label={strings.stepDoneLabel(text)}
                    onClick={() => {
                      mark(i, "done");
                    }}
                  >
                    {strings.stepDone}
                  </button>
                  <button
                    type="button"
                    className="btn btn-soft"
                    aria-label={strings.stepSkipLabel(text)}
                    onClick={() => {
                      mark(i, "skipped");
                    }}
                  >
                    {strings.stepSkip}
                  </button>
                </>
              ) : null}
            </li>
          );
        })}
      </ol>
      {!done && canRun && !run.steps.includes("open") ? (
        <button
          type="button"
          className="btn btn-ink self-start"
          onClick={() => {
            const last = run.steps.length - 1;
            mark(last, run.steps[last] ?? "done", true);
          }}
        >
          {strings.runComplete}
        </button>
      ) : null}
    </li>
  );
}

/** Protocol library and checklist runs (F-CLN-02); the library is signed configuration. */
export function Protocols({ api, crewId, canRun }: { readonly api: Api; readonly crewId: string | null; readonly canRun: boolean }) {
  const [bump, setBump] = useState(0);
  const [notice, setNotice] = useState<Notice>(null);
  const library = useLoad(() => api.protocolList(), "library");
  const runs = useLoad(() => api.checklistList(), String(bump));
  const protocols = library.state === "ok" ? library.data : [];
  const changed = (n: Notice) => {
    setNotice(n);
    setBump((b) => b + 1);
  };
  const start = (p: ProtocolItem) => {
    void api.checklistStart(p.id, crewId, null).then((r) => {
      changed(r.status === "error" ? { kind: "error", text: errorText(r.error) } : null);
    });
  };
  return (
    <>
      <Card label={strings.protocolsTitle}>
        <SectionHead title={strings.protocolsTitle} />
        {library.state === "error" ? <NoticeLine notice={{ kind: "error", text: library.message }} /> : null}
        {library.state === "ok" && protocols.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.protocolsEmpty}</p> : null}
        <ul className="mt-3 flex flex-col gap-2">
          {protocols.map((p) => (
            <li key={p.id} className="flex flex-wrap items-center gap-3">
              <span className="font-semibold">{p.title}</span>
              {canRun ? (
                <button
                  type="button"
                  className="btn btn-soft"
                  aria-label={strings.protocolStartLabel(p.title)}
                  onClick={() => {
                    start(p);
                  }}
                >
                  {strings.protocolStart}
                </button>
              ) : null}
            </li>
          ))}
        </ul>
      </Card>
      <Card label={strings.runsTitle}>
        <SectionHead title={strings.runsTitle} />
        <NoticeLine notice={notice} />
        {runs.state === "ok" && runs.data.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.runsEmpty}</p> : null}
        {runs.state === "ok" ? (
          <ul className="mt-3 flex flex-col gap-3">
            {runs.data.map((r) => (
              <Run key={r.id} api={api} run={r} protocol={protocols.find((p) => p.id === r.protocolId)} canRun={canRun} onChanged={changed} />
            ))}
          </ul>
        ) : null}
      </Card>
    </>
  );
}
