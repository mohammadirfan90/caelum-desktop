import { type SyntheticEvent, useId, useState } from "react";

import type { Api } from "../api";
import type { CrewMember, Encounter, OverviewEntry, RoleName } from "../bindings";
import { level } from "../components/AlarmBar";
import { type Notice, NoticeLine, useConfirmed } from "../components/Confirm";
import { LevelBadge } from "../components/LevelGlyph";
import { useLoad } from "../components/useLoad";
import { Card, NoData, SectionHead } from "../components/Values";
import { strings } from "../strings";
import { utcDateTime } from "../time";
import { errorText } from "./Access";
import { Models } from "./Ground";
import { Protocols } from "./Protocols";
import { DicomViewer, EcgReview } from "./Viewers";

const unitText = (u: string) => strings.unit[u] ?? u;

function OverviewCell({ entry, parameter }: { readonly entry: OverviewEntry; readonly parameter: string }) {
  const v = entry.vitals.find((x) => x.reading.parameter === parameter);
  if (v === undefined || v.reading.value === null) return <NoData status="noData" />;
  if (v.status === "unusable") return <NoData status="unusable" />;
  if (v.status === "stale") return <NoData status="stale" />;
  return <span className="tabular-nums">{`${String(v.reading.value)} ${unitText(v.reading.unit)}`}</span>;
}

/** Crew overview (F-CLN-01): every active crew member, top alarm, key vitals with freshness. */
function Overview({ api, version }: { readonly api: Api; readonly version: number }) {
  const data = useLoad(() => api.crewOverview(), String(version));
  return (
    <Card label={strings.overviewTitle}>
      <SectionHead title={strings.overviewTitle} />
      {data.state === "error" ? <NoticeLine notice={{ kind: "error", text: data.message }} /> : null}
      {data.state === "ok" && data.data.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.overviewEmpty}</p> : null}
      {data.state === "ok" && data.data.length > 0 ? (
        <table className="mt-4 w-full text-left text-sm">
          <thead>
            <tr>
              {Object.values(strings.overviewColumns).map((h) => (
                <th key={h} scope="col">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {data.data.map((c) => (
              <tr key={c.crewMemberId}>
                <th scope="row" className="font-semibold">
                  {c.name}
                </th>
                <td>
                  <LevelBadge level={c.topPriority === null ? "normal" : level(c.topPriority)} />
                </td>
                <td>
                  <OverviewCell entry={c} parameter="heart_rate" />
                </td>
                <td>
                  <OverviewCell entry={c} parameter="spo2" />
                </td>
                <td>
                  <OverviewCell entry={c} parameter="respiration_rate" />
                </td>
                <td>
                  <OverviewCell entry={c} parameter="body_temp_measured" />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
    </Card>
  );
}

type Soap = { subjective: string; objective: string; assessment: string; plan: string };
const EMPTY: Soap = { subjective: "", objective: "", assessment: "", plan: "" };
const orNull = (s: string) => (s.trim() === "" ? null : s);

function Snapshot({ json }: { readonly json: string | null }) {
  let rows: { parameter: string; value: number | null; unit: string }[] = [];
  try {
    const parsed: unknown = JSON.parse(json ?? "[]");
    if (Array.isArray(parsed)) rows = parsed as typeof rows;
  } catch {
    rows = [];
  }
  if (rows.length === 0) return <p className="text-sm text-ink-3">{strings.encounterVitalsNone}</p>;
  return (
    <ul className="text-sm">
      {rows.map((r) => (
        <li key={r.parameter}>{`${strings.parameter[r.parameter] ?? r.parameter}: ${r.value === null ? strings.noData.noData : `${String(r.value)} ${unitText(r.unit)}`}`}</li>
      ))}
    </ul>
  );
}

/** Encounters (F-CLN-05): SOAP draft by the author, signed once with re-authentication. */
function Encounters({ api, crewId, self }: { readonly api: Api; readonly crewId: string; readonly self: string }) {
  const [bump, setBump] = useState(0);
  const [editing, setEditing] = useState<{ id: string | null; soap: Soap } | null>(null);
  const [notice, setNotice] = useState<Notice>(null);
  const { run, prompt } = useConfirmed(api.authReauth);
  const list = useLoad(() => api.encounterList(crewId), `${crewId}:${String(bump)}`);
  const ids = { subjective: useId(), objective: useId(), assessment: useId(), plan: useId() };
  const save = (e: SyntheticEvent) => {
    e.preventDefault();
    if (editing === null) return;
    const s = editing.soap;
    void api.encounterSave(editing.id, crewId, orNull(s.subjective), orNull(s.objective), orNull(s.assessment), orNull(s.plan), []).then((r) => {
      if (r.status === "error") {
        setNotice({ kind: "error", text: errorText(r.error) });
        return;
      }
      setEditing({ id: r.data, soap: s });
      setNotice({ kind: "ok", text: strings.encounterSaved });
      setBump((b) => b + 1);
    });
  };
  const sign = (id: string) => {
    void run(
      () => api.encounterSign(id),
      () => {
        setEditing(null);
        setNotice({ kind: "ok", text: strings.encounterSignedDone });
        setBump((b) => b + 1);
      },
      (err) => {
        setNotice({ kind: "error", text: errorText(err) });
      },
    );
  };
  const open = (e: Encounter) => {
    setEditing({ id: e.id, soap: { subjective: e.subjective ?? "", objective: e.objective ?? "", assessment: e.assessment ?? "", plan: e.plan ?? "" } });
  };
  return (
    <Card label={strings.encountersTitle}>
      <SectionHead title={strings.encountersTitle} />
      {prompt}
      <NoticeLine notice={notice} />
      <button
        type="button"
        className="btn btn-soft mt-3"
        onClick={() => {
          setEditing({ id: null, soap: EMPTY });
        }}
      >
        {strings.encounterNew}
      </button>
      {editing === null ? null : (
        <form onSubmit={save} aria-label={strings.encounterNew} className="mt-4 flex flex-col gap-3">
          {(Object.keys(strings.soap) as (keyof Soap)[]).map((k) => (
            <div key={k} className="flex flex-col gap-1">
              <label htmlFor={ids[k]} className="text-sm font-semibold">
                {strings.soap[k]}
              </label>
              <textarea
                id={ids[k]}
                rows={3}
                className="rounded-lg border border-ink-3 bg-surface p-3"
                value={editing.soap[k]}
                onChange={(ev) => {
                  setEditing({ ...editing, soap: { ...editing.soap, [k]: ev.target.value } });
                }}
              />
            </div>
          ))}
          <div className="flex flex-wrap gap-3">
            <button type="submit" className="btn btn-soft">
              {strings.encounterSave}
            </button>
            {editing.id === null ? null : (
              <button
                type="button"
                className="btn btn-ink"
                onClick={() => {
                  if (editing.id !== null) sign(editing.id);
                }}
              >
                {strings.encounterSign}
              </button>
            )}
          </div>
        </form>
      )}
      {list.state === "error" ? <NoticeLine notice={{ kind: "error", text: list.message }} /> : null}
      {list.state === "ok" && list.data.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.encountersEmpty}</p> : null}
      {list.state === "ok" ? (
        <ul className="mt-4 flex flex-col gap-4">
          {list.data.map((e) => (
            <li key={e.id} className="flex flex-col gap-2 border-t border-border pt-3">
              <p className="text-sm font-semibold">
                {`${utcDateTime(e.createdAtUtcMs)} · ${e.signedAtUtcMs === null ? strings.encounterDraft : strings.encounterSigned(utcDateTime(e.signedAtUtcMs))}`}
              </p>
              <dl className="grid grid-cols-[minmax(7rem,auto)_1fr] gap-x-4 text-sm">
                {(Object.keys(strings.soap) as (keyof Soap)[]).map((k) =>
                  e[k] === null ? null : (
                    <div key={k} className="contents">
                      <dt className="font-semibold">{strings.soap[k]}</dt>
                      <dd className="whitespace-pre-wrap">{e[k]}</dd>
                    </div>
                  ),
                )}
              </dl>
              <h4 className="text-sm font-semibold">{strings.encounterVitals}</h4>
              <Snapshot json={e.vitals} />
              {e.signedAtUtcMs === null && e.author === self ? (
                <button
                  type="button"
                  className="btn btn-soft self-start"
                  aria-label={strings.encounterEditLabel(utcDateTime(e.createdAtUtcMs))}
                  onClick={() => {
                    open(e);
                  }}
                >
                  {strings.encounterEdit}
                </button>
              ) : null}
            </li>
          ))}
        </ul>
      ) : null}
    </Card>
  );
}

/**
 * Attachments (F-CLN-04, F-ACQ-06, F-ACQ-07, F-ACQ-08): import into the encrypted store (DICOM and
 * ECG files must open first; FHIR observations are imported and evaluated), list, and review.
 */
function Attachments({ api, crewId }: { readonly api: Api; readonly crewId: string }) {
  const fileId = useId();
  const kindId = useId();
  const [file, setFile] = useState<File | null>(null);
  const [kind, setKind] = useState("file");
  const [bump, setBump] = useState(0);
  const [notice, setNotice] = useState<Notice>(null);
  const [open, setOpen] = useState<{ id: string; kind: string; name: string } | null>(null);
  const [refused, setRefused] = useState<readonly string[]>([]);
  const list = useLoad(() => api.attachmentList(crewId), `${crewId}:${String(bump)}`);
  const importFile = (e: SyntheticEvent) => {
    e.preventDefault();
    if (file === null) {
      setNotice({ kind: "error", text: strings.attachmentChoose });
      return;
    }
    setRefused([]);
    if (kind === "fhir") {
      void file.text().then((json) =>
        api.fhirImport(crewId, file.name, json).then((r) => {
          if (r.status === "error") setNotice({ kind: "error", text: errorText(r.error) });
          else {
            setNotice({ kind: "ok", text: strings.fhirImported(r.data.imported, r.data.rejected.length) });
            setRefused(r.data.rejected);
            setBump((b) => b + 1);
          }
        }),
      );
      return;
    }
    void file.arrayBuffer().then((buf) =>
      api.attachmentImport(crewId, kind, file.name, Array.from(new Uint8Array(buf))).then((r) => {
        if (r.status === "error") setNotice({ kind: "error", text: errorText(r.error) });
        else {
          setNotice({ kind: "ok", text: strings.attachmentImported(file.name) });
          setBump((b) => b + 1);
        }
      }),
    );
  };
  return (
    <Card label={strings.attachmentsTitle}>
      <SectionHead title={strings.attachmentsTitle} />
      <form onSubmit={importFile} aria-label={strings.attachmentImport} className="mt-3 flex flex-wrap items-end gap-3">
        <div className="flex flex-col gap-1">
          <label htmlFor={fileId} className="text-sm font-semibold">
            {strings.attachmentFile}
          </label>
          <input
            id={fileId}
            type="file"
            className="min-h-12"
            onChange={(e) => {
              setFile(e.target.files?.[0] ?? null);
            }}
          />
        </div>
        <div className="flex flex-col gap-1">
          <label htmlFor={kindId} className="text-sm font-semibold">
            {strings.attachmentKind}
          </label>
          <select
            id={kindId}
            className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
            value={kind}
            onChange={(e) => {
              setKind(e.target.value);
            }}
          >
            {Object.entries(strings.attachmentKinds).map(([k, v]) => (
              <option key={k} value={k}>
                {v}
              </option>
            ))}
          </select>
        </div>
        <button type="submit" className="btn btn-ink">
          {strings.attachmentImport}
        </button>
      </form>
      <NoticeLine notice={notice} />
      {refused.length > 0 ? (
        <div className="mt-2 text-sm">
          <p className="font-semibold">{strings.fhirRejectedTitle}</p>
          <ul className="list-disc pl-5">
            {refused.map((r) => (
              <li key={r}>{r}</li>
            ))}
          </ul>
        </div>
      ) : null}
      {list.state === "ok" && list.data.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.attachmentsEmpty}</p> : null}
      {list.state === "ok" && list.data.length > 0 ? (
        <ul className="mt-3 flex flex-col gap-2 text-sm">
          {list.data.map((a) => {
            const name = a.filename ?? a.id;
            return (
              <li key={a.id} className="flex flex-wrap items-center gap-3">
                <span>{`${name} · ${strings.attachmentKinds[a.kind] ?? a.kind} · ${utcDateTime(a.createdAtUtcMs)}`}</span>
                {a.kind === "ecg" || a.kind === "dicom" ? (
                  <button
                    type="button"
                    className="btn btn-soft"
                    onClick={() => {
                      setOpen({ id: a.id, kind: a.kind, name });
                    }}
                  >
                    {strings.attachmentReview(name)}
                  </button>
                ) : null}
              </li>
            );
          })}
        </ul>
      ) : null}
      {open?.kind === "ecg" ? (
        <EcgReview
          key={open.id}
          api={api}
          id={open.id}
          name={open.name}
          onClose={() => {
            setOpen(null);
          }}
        />
      ) : null}
      {open?.kind === "dicom" ? (
        <DicomViewer
          key={open.id}
          api={api}
          id={open.id}
          name={open.name}
          onClose={() => {
            setOpen(null);
          }}
        />
      ) : null}
    </Card>
  );
}

/** Medication and medical-kit log (F-CLN-08): the CMO records; others read. */
function Medication({ api, userRole, crew }: { readonly api: Api; readonly userRole: RoleName; readonly crew: readonly CrewMember[] }) {
  const ids = { kind: useId(), item: useId(), amount: useId(), unit: useId(), note: useId(), crew: useId() };
  const [form, setForm] = useState({ kind: "dose", item: "", amount: "", unit: "", note: "", crew: "" });
  const [bump, setBump] = useState(0);
  const [notice, setNotice] = useState<Notice>(null);
  const list = useLoad(() => api.medicationLog(), String(bump));
  const crewName = (id: string | null) => (id === null ? "" : (crew.find((c) => c.id === id)?.name ?? id));
  const add = (e: SyntheticEvent) => {
    e.preventDefault();
    const amount = form.amount.trim() === "" ? null : Number(form.amount);
    void api
      .medicationAdd(form.kind === "dose" && form.crew !== "" ? form.crew : null, form.kind, form.item, amount, orNull(form.unit), orNull(form.note))
      .then((r) => {
        if (r.status === "error") setNotice({ kind: "error", text: errorText(r.error) });
        else {
          setNotice({ kind: "ok", text: strings.medicationAdded });
          setForm({ ...form, item: "", amount: "", unit: "", note: "" });
          setBump((b) => b + 1);
        }
      });
  };
  const field = (key: "item" | "amount" | "unit" | "note", label: string) => (
    <div className="flex flex-col gap-1">
      <label htmlFor={ids[key]} className="text-sm font-semibold">
        {label}
      </label>
      <input
        id={ids[key]}
        inputMode={key === "amount" ? "decimal" : undefined}
        className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
        value={form[key]}
        onChange={(e) => {
          setForm({ ...form, [key]: e.target.value });
        }}
      />
    </div>
  );
  return (
    <Card label={strings.medicationTitle}>
      <SectionHead title={strings.medicationTitle} />
      {userRole === "cmo" ? (
        <form onSubmit={add} aria-label={strings.medicationAdd} className="mt-3 flex flex-wrap items-end gap-3">
          <div className="flex flex-col gap-1">
            <label htmlFor={ids.kind} className="text-sm font-semibold">
              {strings.medicationKind}
            </label>
            <select
              id={ids.kind}
              className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
              value={form.kind}
              onChange={(e) => {
                setForm({ ...form, kind: e.target.value });
              }}
            >
              {Object.entries(strings.medicationKinds).map(([k, v]) => (
                <option key={k} value={k}>
                  {v}
                </option>
              ))}
            </select>
          </div>
          {form.kind === "dose" ? (
            <div className="flex flex-col gap-1">
              <label htmlFor={ids.crew} className="text-sm font-semibold">
                {strings.medicationFor}
              </label>
              <select
                id={ids.crew}
                className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
                value={form.crew}
                onChange={(e) => {
                  setForm({ ...form, crew: e.target.value });
                }}
              >
                <option value="">{strings.crewSelectNone}</option>
                {crew
                  .filter((c) => c.active)
                  .map((c) => (
                    <option key={c.id} value={c.id}>
                      {c.name}
                    </option>
                  ))}
              </select>
            </div>
          ) : null}
          {field("item", strings.medicationItem)}
          {field("amount", strings.medicationAmount)}
          {field("unit", strings.medicationUnit)}
          {field("note", strings.medicationNote)}
          <button type="submit" className="btn btn-ink">
            {strings.medicationAdd}
          </button>
        </form>
      ) : null}
      <NoticeLine notice={notice} />
      {list.state === "ok" && list.data.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.medicationEmpty}</p> : null}
      {list.state === "ok" && list.data.length > 0 ? (
        <table className="mt-4 w-full text-left text-sm">
          <thead>
            <tr>
              {Object.values(strings.medicationColumns).map((h) => (
                <th key={h} scope="col">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {list.data.map((m) => (
              <tr key={m.id}>
                <td>{utcDateTime(m.atUtcMs)}</td>
                <td>{strings.medicationKinds[m.kind] ?? m.kind}</td>
                <td>{m.item}</td>
                <td>{m.amount === null ? "" : `${String(m.amount)} ${m.unit ?? ""}`}</td>
                <td>{crewName(m.crewMemberId)}</td>
                <td>{m.note ?? ""}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}
    </Card>
  );
}

/** Check-ins the crew member shared with medical (F-CLN-07; private ones never reach here). */
function SharedCheckins({ api, crewId }: { readonly api: Api; readonly crewId: string }) {
  const list = useLoad(() => api.checkinList(crewId), crewId);
  return (
    <Card label={strings.checkinsTitle}>
      <SectionHead title={strings.checkinsTitle} />
      {list.state === "ok" && list.data.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.checkinsEmpty}</p> : null}
      {list.state === "ok" && list.data.length > 0 ? (
        <ul className="mt-3 flex flex-col gap-1 text-sm">
          {list.data.map((c) => (
            <li key={c.id}>
              {`${utcDateTime(c.atUtcMs)} · `}
              {c.answers === null
                ? strings.noData.noData
                : strings.checkinRow(
                    c.answers.mood,
                    c.answers.sleepHours === null ? strings.noData.noData : strings.hours(c.answers.sleepHours),
                    c.answers.symptoms.length === 0 ? strings.noSymptoms : c.answers.symptoms.join(", "),
                  )}
            </li>
          ))}
        </ul>
      ) : null}
    </Card>
  );
}

const EXPORT_S = 7 * 24 * 3600;
const EXPORT_PARAMS = ["heart_rate", "spo2", "respiration_rate", "body_temp_measured", "bp_systolic", "bp_diastolic", "weight", "pvt_median_rt"];

/** Saves text the core produced as a file (no file-system access is granted to the window). */
function download(name: string, type: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  URL.revokeObjectURL(url);
}

/** Export (F-SYS-08): FHIR R4 bundle and per-parameter CSV, both audited. */
function Export({ api, crewId }: { readonly api: Api; readonly crewId: string }) {
  const paramId = useId();
  const [parameter, setParameter] = useState("heart_rate");
  const [notice, setNotice] = useState<Notice>(null);
  const run = (name: string, type: string, p: ReturnType<Api["exportFhir"]>) => {
    void p.then((r) => {
      if (r.status === "error") setNotice({ kind: "error", text: errorText(r.error) });
      else {
        download(name, type, r.data);
        setNotice({ kind: "ok", text: strings.exportDone(name) });
      }
    });
  };
  return (
    <Card label={strings.exportTitle}>
      <SectionHead title={strings.exportTitle} />
      <p className="mt-2 text-sm text-ink-3">{strings.exportHelp}</p>
      <div className="mt-3 flex flex-wrap items-end gap-3">
        <button
          type="button"
          className="btn btn-soft"
          onClick={() => {
            run(`caelum-${crewId}.fhir.json`, "application/fhir+json", api.exportFhir(crewId, EXPORT_S));
          }}
        >
          {strings.exportFhir}
        </button>
        <div className="flex flex-col gap-1">
          <label htmlFor={paramId} className="text-sm font-semibold">
            {strings.exportParameter}
          </label>
          <select
            id={paramId}
            className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
            value={parameter}
            onChange={(e) => {
              setParameter(e.target.value);
            }}
          >
            {EXPORT_PARAMS.map((p) => (
              <option key={p} value={p}>
                {strings.parameter[p] ?? p}
              </option>
            ))}
          </select>
        </div>
        <button
          type="button"
          className="btn btn-soft"
          onClick={() => {
            run(`caelum-${crewId}-${parameter}.csv`, "text/csv", api.exportCsv(crewId, parameter, EXPORT_S));
          }}
        >
          {strings.exportCsv}
        </button>
      </div>
      <NoticeLine notice={notice} />
    </Card>
  );
}

/** Medical (docs/07 §4): crew overview, encounters, attachments, check-ins, medication, protocols. */
export function Medical({
  api,
  userRole,
  self,
  crewId,
  crew,
  version,
}: {
  readonly api: Api;
  readonly userRole: RoleName;
  readonly self: string;
  readonly crewId: string | null;
  readonly crew: readonly CrewMember[];
  readonly version: number;
}) {
  return (
    <div className="flex flex-col gap-6">
      <Overview api={api} version={version} />
      {crewId === null ? (
        <p className="text-sm text-ink-3">{strings.chooseCrew}</p>
      ) : (
        <>
          <Encounters api={api} crewId={crewId} self={self} />
          <Attachments api={api} crewId={crewId} />
          <SharedCheckins api={api} crewId={crewId} />
          <Export api={api} crewId={crewId} />
        </>
      )}
      <Medication api={api} userRole={userRole} crew={crew} />
      <Protocols api={api} crewId={crewId} canRun={userRole === "cmo"} />
      <Models api={api} />
    </div>
  );
}
