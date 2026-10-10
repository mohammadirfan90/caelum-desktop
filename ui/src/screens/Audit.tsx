import type { Api } from "../api";
import { NoticeLine } from "../components/Confirm";
import { LevelGlyph } from "../components/LevelGlyph";
import { useLoad } from "../components/useLoad";
import { Card, SectionHead } from "../components/Values";
import { strings } from "../strings";
import { utcDateTime } from "../time";

/** Audit (docs/07 §4, F-SYS-03): the signed audit log the role may read, with chain status. */
export function Audit({ api, version }: { readonly api: Api; readonly version: number }) {
  const key = String(version);
  const status = useLoad(() => api.auditVerify(), key);
  const list = useLoad(() => api.auditList(), key);
  return (
    <div className="flex flex-col gap-6">
      <Card label={strings.auditTitle}>
        <SectionHead title={strings.auditTitle} />
        {status.state === "error" ? <NoticeLine notice={{ kind: "error", text: status.message }} /> : null}
        {status.state === "ok" ? (
          status.data.intact ? (
            <p role="status" className="mt-3 font-semibold">
              {strings.auditIntact(status.data.entries)}
            </p>
          ) : (
            <div role="alert" className="mt-3 flex flex-col gap-2">
              <p className="flex items-center gap-2 font-semibold">
                <LevelGlyph level="critical" />
                {strings.auditBroken(status.data.entries)}
              </p>
              <ul className="text-sm">
                {status.data.problems.map((p) => (
                  <li key={`${p.seq}/${p.kind}`}>{strings.auditProblemAt(p.seq, strings.auditProblem[p.kind] ?? p.kind)}</li>
                ))}
              </ul>
            </div>
          )
        ) : null}
      </Card>
      <Card label={strings.auditTitle}>
        {list.state === "error" ? <NoticeLine notice={{ kind: "error", text: list.message }} /> : null}
        {list.state === "ok" && list.data.length === 0 ? <p className="text-sm text-ink-3">{strings.auditEmpty}</p> : null}
        {list.state === "ok" && list.data.length > 0 ? (
          <table className="w-full text-left text-sm">
            <thead>
              <tr>
                {Object.values(strings.auditColumns).map((h) => (
                  <th key={h} scope="col">
                    {h}
                  </th>
                ))}
              </tr>
            </thead>
            <tbody>
              {[...list.data].reverse().map((e) => (
                <tr key={e.id}>
                  <td>{e.seq}</td>
                  <td>{utcDateTime(e.atUtcMs)}</td>
                  <td>{e.role}</td>
                  <td>{e.action}</td>
                  <td className="break-all">{e.target ?? ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        ) : null}
      </Card>
    </div>
  );
}
