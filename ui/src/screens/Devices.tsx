import { useId, useState } from "react";

import type { Api } from "../api";
import type { CrewMember, DeviceEntry, NearbyDevice, RoleName } from "../bindings";
import { type Notice, NoticeLine, useConfirmed } from "../components/Confirm";
import { useLoad } from "../components/useLoad";
import { Card, NoData, SectionHead } from "../components/Values";
import { strings } from "../strings";
import { utcDateTime } from "../time";
import { errorText } from "./Access";

const MANAGERS: readonly RoleName[] = ["cmo", "bme", "administrator"];

function AssignControl({
  device,
  crew,
  onAssign,
}: {
  readonly device: DeviceEntry;
  readonly crew: readonly CrewMember[];
  readonly onAssign: (crewId: string) => void;
}) {
  const id = useId();
  const [choice, setChoice] = useState("");
  return (
    <span className="flex flex-wrap items-center gap-2">
      <label htmlFor={id} className="sr-only">
        {strings.assignTo(device.model)}
      </label>
      <select
        id={id}
        className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
        value={choice}
        onChange={(e) => {
          setChoice(e.target.value);
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
      <button
        type="button"
        className="btn btn-soft"
        aria-label={strings.assignLabel(device.model)}
        disabled={choice === ""}
        onClick={() => {
          onAssign(choice);
        }}
      >
        {strings.assign}
      </button>
    </span>
  );
}

function Scan({ api, onPaired }: { readonly api: Api; readonly onPaired: () => void }) {
  const [found, setFound] = useState<NearbyDevice[] | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<Notice>(null);
  const [steps, setSteps] = useState<string[]>([]);
  const scan = () => {
    setBusy(true);
    setNotice(null);
    void api.deviceScan().then((r) => {
      setBusy(false);
      if (r.status === "ok") setFound(r.data);
      else setNotice({ kind: "error", text: errorText(r.error) });
    });
  };
  const pair = (d: NearbyDevice) => {
    const name = d.name ?? strings.unnamed;
    void api.devicePair(d.address, d.name, d.services).then((r) => {
      if (r.status === "error") {
        setNotice({ kind: "error", text: errorText(r.error) });
      } else if (r.data.result === "registered") {
        setSteps([]);
        setNotice({ kind: "ok", text: strings.paired(name) });
        onPaired();
      } else {
        setSteps(r.data.steps);
        setNotice({ kind: "error", text: `${strings.pairOs} ${r.data.reason}` });
      }
    });
  };
  return (
    <Card label={strings.scanTitle}>
      <SectionHead title={strings.scanTitle} />
      <div className="mt-3 flex flex-col gap-3">
        <button type="button" className="btn btn-ink self-start" disabled={busy} onClick={scan}>
          {busy ? strings.scanning : strings.scan}
        </button>
        <NoticeLine notice={notice} />
        {steps.length > 0 ? (
          <ol className="list-decimal pl-6 text-sm">
            {steps.map((s) => (
              <li key={s}>{s}</li>
            ))}
          </ol>
        ) : null}
        {found !== null && found.length === 0 ? <p className="text-sm text-ink-3">{strings.scanEmpty}</p> : null}
        {found !== null && found.length > 0 ? (
          <ul className="flex flex-col gap-2">
            {found.map((d) => {
              const name = d.name ?? strings.unnamed;
              return (
                <li key={d.address} className="flex flex-wrap items-center gap-3">
                  <span className="font-semibold">{d.emulated ? `${name} (${strings.emulated})` : name}</span>
                  <span className="text-sm text-ink-3">{d.address}</span>
                  <button
                    type="button"
                    className="btn btn-soft"
                    aria-label={strings.pairLabel(name)}
                    onClick={() => {
                      pair(d);
                    }}
                  >
                    {strings.pair}
                  </button>
                </li>
              );
            })}
          </ul>
        ) : null}
      </div>
    </Card>
  );
}

/** Devices (docs/07 §4; F-ACQ-01, F-ACQ-09, F-ACQ-10): registry, link health, pairing, assignment. */
export function Devices({
  api,
  userRole: role,
  crew,
  version,
}: {
  readonly api: Api;
  readonly userRole: RoleName;
  readonly crew: readonly CrewMember[];
  readonly version: number;
}) {
  const [bump, setBump] = useState(0);
  const [notice, setNotice] = useState<Notice>(null);
  const { run, prompt } = useConfirmed(api.authReauth);
  const list = useLoad(() => api.deviceList(), `${String(version)}:${String(bump)}`);
  const manage = MANAGERS.includes(role);
  const crewName = (id: string | null) => (id === null ? strings.unassigned : (crew.find((c) => c.id === id)?.name ?? id));
  const reload = () => {
    setBump((b) => b + 1);
  };
  const assign = (d: DeviceEntry, crewId: string) => {
    void run(
      () => api.deviceAssign(d.id, crewId, null),
      () => {
        setNotice({ kind: "ok", text: strings.assigned(d.model, crewName(crewId)) });
        reload();
      },
      (e) => {
        setNotice({ kind: "error", text: errorText(e) });
      },
    );
  };
  const unassign = (d: DeviceEntry) => {
    void run(
      () => api.deviceUnassign(d.id),
      () => {
        setNotice({ kind: "ok", text: strings.unassignedDone(d.model) });
        reload();
      },
      (e) => {
        setNotice({ kind: "error", text: errorText(e) });
      },
    );
  };
  return (
    <div className="flex flex-col gap-6">
      {prompt}
      <Card label={strings.devicesTitle}>
        <SectionHead title={strings.devicesTitle} />
        <NoticeLine notice={notice} />
        {list.state === "error" ? <NoticeLine notice={{ kind: "error", text: list.message }} /> : null}
        {list.state === "ok" && list.data.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.devicesEmpty}</p> : null}
        {list.state === "ok" && list.data.length > 0 ? (
          <table className="mt-4 w-full text-left text-sm">
            <thead>
              <tr>
                {Object.values(strings.deviceColumns)
                  .slice(0, manage ? 8 : 7)
                  .map((h) => (
                    <th key={h} scope="col">
                      {h}
                    </th>
                  ))}
              </tr>
            </thead>
            <tbody>
              {list.data.map((d) => (
                <tr key={d.id}>
                  <td>
                    <span className="font-semibold">{d.model}</span>
                    <span className="block text-xs text-ink-3">{d.serial}</span>
                  </td>
                  <td>{strings.adapter[d.adapter] ?? d.adapter}</td>
                  <td>{crewName(d.crewMemberId)}</td>
                  <td>{strings.linkStatus[d.status] ?? d.status}</td>
                  <td>{d.batteryPct === null ? <NoData status="noData" /> : strings.pct(d.batteryPct)}</td>
                  <td>{d.lastSeenUtcMs === null ? <NoData status="noData" /> : utcDateTime(d.lastSeenUtcMs)}</td>
                  <td>{d.clockOffsetMs === null ? <NoData status="noData" /> : strings.ms(d.clockOffsetMs)}</td>
                  {manage ? (
                    <td>
                      {d.crewMemberId === null ? (
                        <AssignControl
                          device={d}
                          crew={crew}
                          onAssign={(c) => {
                            assign(d, c);
                          }}
                        />
                      ) : (
                        <button
                          type="button"
                          className="btn btn-soft"
                          aria-label={strings.unassignLabel(d.model)}
                          onClick={() => {
                            unassign(d);
                          }}
                        >
                          {strings.unassign}
                        </button>
                      )}
                    </td>
                  ) : null}
                </tr>
              ))}
            </tbody>
          </table>
        ) : null}
      </Card>
      {manage ? <Scan api={api} onPaired={reload} /> : null}
    </div>
  );
}
