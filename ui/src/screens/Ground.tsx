import type { Api } from "../api";
import { NoticeLine } from "../components/Confirm";
import { useLoad } from "../components/useLoad";
import { Card, NoData, SectionHead } from "../components/Values";
import { strings } from "../strings";

/** Ground (docs/07 §4; F-GND-11, SR-015): path B to the vehicle and the inbound vehicle bridge. */
export function Ground({ api, version }: { readonly api: Api; readonly version: number }) {
  const status = useLoad(() => api.groundStatus(), String(version));
  const mds = useLoad(() => api.mdsStatus(), String(version));
  if (status.state === "error") return <NoticeLine notice={{ kind: "error", text: status.message }} />;
  if (status.state !== "ok") return null;
  const s = status.data;
  const v = s.vehicle;
  return (
    <div className="flex flex-col gap-6">
      <Card label={strings.pathBTitle}>
        <SectionHead title={strings.pathBTitle} />
        <p className="mt-3 font-semibold">{s.pathBConfigured && s.groundVersion !== null ? strings.pathBConfigured(s.groundVersion) : strings.pathBNotConfigured}</p>
        {s.pathBProblem === null ? null : <p className="text-sm">{s.pathBProblem}</p>}
        {s.pathBConfigured ? <p className="text-sm">{strings.pathBWaiting(s.pathBWaiting)}</p> : null}
      </Card>
      <Card label={strings.mdsTitle}>
        <SectionHead title={strings.mdsTitle} />
        {mds.state === "error" ? <NoticeLine notice={{ kind: "error", text: mds.message }} /> : null}
        {mds.state === "ok" ? (
          <>
            <p className="mt-3 font-semibold">{strings.mdsHealth[mds.data.health] ?? mds.data.health}</p>
            {mds.data.problem === null ? null : <p className="text-sm">{mds.data.problem}</p>}
            {mds.data.configured ? <p className="text-sm">{strings.mdsCounts(mds.data.pending, mds.data.held)}</p> : null}
          </>
        ) : null}
      </Card>
      <Card label={strings.vehicleTitle}>
        <SectionHead title={strings.vehicleTitle} />
        <p className="mt-3 font-semibold">{v.running && v.version !== null ? strings.vehicleRunning(v.version) : strings.vehicleStopped}</p>
        {v.problem === null ? null : <p className="text-sm">{v.problem}</p>}
        <p className="text-sm">{strings.vehicleCounts(v.datagrams, v.readings)}</p>
        <p className="text-sm">{v.clock === null ? <NoData status="noData" /> : strings.vehicleClock(strings.clockStates[v.clock] ?? v.clock)}</p>
        {v.latestOffsetMs === null ? null : <p className="text-sm">{strings.vehicleOffset(v.latestOffsetMs)}</p>}
        {v.rejected.length === 0 ? null : (
          <ul className="text-sm">
            {v.rejected.map(([code, n]) => (
              <li key={code}>{strings.vehicleDropped(code, n)}</li>
            ))}
          </ul>
        )}
      </Card>
    </div>
  );
}

/** Advisory models and their validation status (F-ML-01); outputs never change alarm rules. */
export function Models({ api }: { readonly api: Api }) {
  const list = useLoad(() => api.modelList(), "models");
  return (
    <Card label={strings.modelsTitle}>
      <SectionHead title={strings.modelsTitle} />
      {list.state === "error" ? <NoticeLine notice={{ kind: "error", text: list.message }} /> : null}
      {list.state === "ok" && list.data.length === 0 ? <p className="mt-3 text-sm text-ink-3">{strings.modelsEmpty}</p> : null}
      {list.state === "ok" && list.data.length > 0 ? (
        <ul className="mt-3 flex flex-col gap-2">
          {list.data.map((m) => (
            <li key={m.id}>
              <span className="font-semibold">{`${m.name} ${m.version}`}</span>
              <span className="ml-2 text-sm">{strings.modelAdvisory(strings.modelStatus[m.status] ?? m.status)}</span>
              {m.developmentSigned ? <span className="ml-2 text-sm font-semibold">{strings.modelDevelopment}</span> : null}
              {m.reason === null ? null : <span className="block text-sm text-ink-3">{m.reason}</span>}
            </li>
          ))}
        </ul>
      ) : null}
    </Card>
  );
}
