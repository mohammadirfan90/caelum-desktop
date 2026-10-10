import { type ReactNode, type SyntheticEvent, useEffect, useId, useRef, useState } from "react";

import type { Api } from "../api";
import type { DicomImage, DicomWindow, EcgLead, EcgRecordingView } from "../bindings";
import { NoticeLine } from "../components/Confirm";
import { useLoad } from "../components/useLoad";
import { Card, SectionHead } from "../components/Values";
import { strings } from "../strings";

const WINDOW_S = 10;
const COLUMNS = 1000;
const field = "min-h-12 rounded-lg border border-ink-3 bg-surface px-3";

function Labelled({ label, children }: { readonly label: string; readonly children: (id: string) => ReactNode }) {
  const id = useId();
  return (
    <div className="flex flex-col gap-1">
      <label htmlFor={id} className="text-sm font-semibold">
        {label}
      </label>
      {children(id)}
    </div>
  );
}

/** Min/max per column so every peak in the window is drawn, however long the recording. */
function trace(samples: readonly (number | null)[], from: number, to: number, lo: number, span: number): string {
  const n = Math.max(1, to - from);
  const out: string[] = [];
  const y = (v: number) => (190 - ((v - lo) / span) * 180).toFixed(1);
  for (let c = 0; c < COLUMNS; c++) {
    const a = from + Math.floor((c * n) / COLUMNS);
    const b = Math.max(a + 1, from + Math.floor(((c + 1) * n) / COLUMNS));
    let mn = Infinity;
    let mx = -Infinity;
    for (let i = a; i < b && i < to; i++) {
      const v = samples[i];
      if (v === null || v === undefined) continue;
      mn = Math.min(mn, v);
      mx = Math.max(mx, v);
    }
    if (mn === Infinity) continue;
    out.push(`${String(c)},${y(mn)}`, `${String(c)},${y(mx)}`);
  }
  return out.join(" ");
}

function valueAt(lead: EcgLead, rate: number, t: number): string {
  const v = lead.samples[Math.round(t * rate)];
  return v === null || v === undefined ? strings.noData.noData : `${v.toFixed(3)} ${lead.unit}`;
}

function EcgPlot({ rec, onExport }: { readonly rec: EcgRecordingView; readonly onExport: ((from: number, to: number) => void) | null }) {
  const [leadIx, setLeadIx] = useState(0);
  const [start, setStart] = useState(0);
  const [a, setA] = useState(0);
  const [b, setB] = useState(1);
  const lead = rec.leads[leadIx] ?? rec.leads[0];
  if (lead === undefined || lead.sampleRateHz === null || lead.sampleRateHz <= 0) return <p className="mt-3 text-sm text-ink-3">{strings.ecgNoLeads}</p>;
  const rate = lead.sampleRateHz;
  const total = lead.samples.length / rate;
  const span = Math.min(WINDOW_S, total);
  const from = Math.round(start * rate);
  const to = Math.min(lead.samples.length, Math.round((start + span) * rate));
  const shown = lead.samples.slice(from, to).filter((v): v is number => v !== null);
  const lo = shown.length > 0 ? Math.min(...shown) : 0;
  const amp = (shown.length > 0 ? Math.max(...shown) : 1) - lo || 1;
  const step = 1 / rate;
  const clampT = (t: number) => Math.min(Math.max(t, start), start + span);
  const ta = clampT(a);
  const tb = clampT(b);
  const x = (t: number) => (((t - start) / span) * COLUMNS).toFixed(1);
  const dt = Math.abs(tb - ta) * 1000;
  const grid: number[] = [];
  for (let t = Math.ceil(start * 5) / 5; t <= start + span; t += 0.2) grid.push(t);
  return (
    <div className="mt-3 flex flex-col gap-3">
      <p className="text-sm">{strings.ecgFormat(rec.format, rec.leads.length, lead.sampleRateHz, total)}</p>
      <div className="flex flex-wrap items-end gap-3">
        <Labelled label={strings.ecgLead}>
          {(id) => (
            <select
              id={id}
              className={field}
              value={leadIx}
              onChange={(e) => {
                setLeadIx(Number(e.target.value));
              }}
            >
              {rec.leads.map((l, i) => (
                <option key={`${l.label}-${String(i)}`} value={i}>
                  {l.label}
                </option>
              ))}
            </select>
          )}
        </Labelled>
        {total > WINDOW_S ? (
          <Labelled label={strings.ecgWindowStart}>
            {(id) => (
              <input
                id={id}
                type="range"
                className="min-h-12"
                min={0}
                max={total - span}
                step={0.5}
                value={start}
                aria-valuetext={start.toFixed(1)}
                onChange={(e) => {
                  const s = Number(e.target.value);
                  setStart(s);
                  setA(s);
                  setB(s + Math.min(1, span));
                }}
              />
            )}
          </Labelled>
        ) : null}
        <Labelled label={strings.ecgCaliperA}>
          {(id) => (
            <input
              id={id}
              type="range"
              className="min-h-12"
              min={start}
              max={start + span}
              step={step}
              value={ta}
              aria-valuetext={ta.toFixed(3)}
              onChange={(e) => {
                setA(Number(e.target.value));
              }}
            />
          )}
        </Labelled>
        <Labelled label={strings.ecgCaliperB}>
          {(id) => (
            <input
              id={id}
              type="range"
              className="min-h-12"
              min={start}
              max={start + span}
              step={step}
              value={tb}
              aria-valuetext={tb.toFixed(3)}
              onChange={(e) => {
                setB(Number(e.target.value));
              }}
            />
          )}
        </Labelled>
      </div>
      <svg role="img" aria-label={strings.ecgTrace(lead.label, start, start + span)} viewBox={`0 0 ${String(COLUMNS)} 200`} className="h-56 w-full" preserveAspectRatio="none">
        {grid.map((t) => (
          <line key={t.toFixed(2)} x1={x(t)} x2={x(t)} y1="0" y2="200" className="stroke-ink-3" strokeWidth="1" opacity="0.25" vectorEffect="non-scaling-stroke" />
        ))}
        <polyline points={trace(lead.samples, from, to, lo, amp)} fill="none" stroke="currentColor" strokeWidth="1.5" vectorEffect="non-scaling-stroke" />
        <line x1={x(ta)} x2={x(ta)} y1="0" y2="200" stroke="currentColor" strokeDasharray="6 4" strokeWidth="2" vectorEffect="non-scaling-stroke" />
        <line x1={x(tb)} x2={x(tb)} y1="0" y2="200" stroke="currentColor" strokeDasharray="2 3" strokeWidth="2" vectorEffect="non-scaling-stroke" />
      </svg>
      {onExport !== null && rec.startKnown ? (
        <button
          type="button"
          className="btn btn-soft self-start"
          onClick={() => {
            onExport(Math.min(ta, tb), Math.max(ta, tb));
          }}
        >
          {strings.ecgExport}
        </button>
      ) : null}
      {onExport !== null && !rec.startKnown ? <p className="text-sm text-ink-3">{strings.ecgExportNoStart}</p> : null}
      {rec.events.length > 0 ? (
        <div className="text-sm">
          <p className="font-semibold">{strings.ecgEvents}</p>
          <ul>
            {rec.events.map((e) => (
              <li key={`${String(e.onsetS)}-${e.text}`}>{strings.ecgEvent(e.onsetS ?? 0, e.text)}</li>
            ))}
          </ul>
        </div>
      ) : null}
      <p className="text-sm tabular-nums" aria-live="polite">
        {`${strings.ecgInterval(dt)} ${dt > 0 ? strings.ecgRate(60000 / dt) : ""} · ${strings.ecgAmplitude(strings.ecgCaliperA, valueAt(lead, rate, ta))} · ${strings.ecgAmplitude(strings.ecgCaliperB, valueAt(lead, rate, tb))}`}
      </p>
    </div>
  );
}

/** ECG review (F-CLN-03): an imported recording, one lead at a time, with time calipers. */
export function EcgReview({ api, id, name, onClose }: { readonly api: Api; readonly id: string; readonly name: string; readonly onClose: () => void }) {
  const data = useLoad(() => api.ecgView(id), id);
  const [notice, setNotice] = useState<{ kind: "ok" | "error"; text: string } | null>(null);
  const exportSegment = (from: number, to: number) => {
    void api.ecgExportSegment(id, from, to).then((r) => {
      if (r.status === "error") {
        setNotice({ kind: "error", text: r.error.message });
        return;
      }
      const file = `${name.replace(/\.[^.]*$/, "")}-${from.toFixed(1)}-${to.toFixed(1)}.edf`;
      const url = URL.createObjectURL(new Blob([new Uint8Array(r.data)], { type: "application/octet-stream" }));
      const a = document.createElement("a");
      a.href = url;
      a.download = file;
      a.click();
      URL.revokeObjectURL(url);
      setNotice({ kind: "ok", text: strings.ecgExported(file) });
    });
  };
  return (
    <Card label={strings.ecgReviewTitle(name)}>
      <SectionHead title={strings.ecgReviewTitle(name)} />
      {data.state === "error" ? <NoticeLine notice={{ kind: "error", text: data.message }} /> : null}
      {data.state === "ok" ? <EcgPlot rec={data.data} onExport={exportSegment} /> : null}
      <NoticeLine notice={notice} />
      <button type="button" className="btn btn-soft mt-3" onClick={onClose}>
        {strings.viewerClose}
      </button>
    </Card>
  );
}

function DicomCanvas({ image }: { readonly image: DicomImage }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const canvas = ref.current;
    // A WebView without a 2D context shows the text description only.
    let ctx: CanvasRenderingContext2D | null = null;
    try {
      ctx = canvas?.getContext("2d") ?? null;
    } catch {
      ctx = null;
    }
    if (ctx === null) return;
    const n = image.rows * image.columns;
    const rgba = new Uint8ClampedArray(n * 4);
    for (let i = 0; i < n; i++) {
      const k = i * image.channels;
      const r = image.pixels[k] ?? 0;
      rgba[i * 4] = r;
      rgba[i * 4 + 1] = image.channels === 3 ? (image.pixels[k + 1] ?? 0) : r;
      rgba[i * 4 + 2] = image.channels === 3 ? (image.pixels[k + 2] ?? 0) : r;
      rgba[i * 4 + 3] = 255;
    }
    ctx.putImageData(new ImageData(rgba, image.columns, image.rows), 0, 0);
  }, [image]);
  const w = image.window;
  const spacing = image.pixelSpacingMm;
  const description = [
    strings.dicomImage(image.rows, image.columns, image.frame, image.frames),
    w !== null && w.center !== null && w.width !== null ? strings.dicomWindow(w.center, w.width) : null,
  ]
    .filter((x) => x !== null)
    .join(" · ");
  return (
    <div className="mt-3 flex flex-col gap-2">
      <div role="img" aria-label={description} className="self-start">
        <canvas ref={ref} width={image.columns} height={image.rows} aria-hidden="true" className="max-h-[60vh] max-w-full bg-black [image-rendering:pixelated]" />
      </div>
      <p className="text-sm tabular-nums">{description}</p>
      <p className="text-sm text-ink-3">
        {spacing !== null && spacing[0] !== null && spacing[1] !== null ? strings.dicomSpacing(spacing[0], spacing[1]) : strings.dicomNoSpacing}
      </p>
    </div>
  );
}

/** DICOM viewer (F-CLN-04): one frame at a time, windowed by the file's presets or by hand. */
export function DicomViewer({ api, id, name, onClose }: { readonly api: Api; readonly id: string; readonly name: string; readonly onClose: () => void }) {
  const [frame, setFrame] = useState(0);
  const [win, setWin] = useState<DicomWindow | null>(null);
  const [center, setCenter] = useState("");
  const [width, setWidth] = useState("");
  const data = useLoad(() => api.dicomView(id, frame, win), `${id}:${String(frame)}:${JSON.stringify(win)}`);
  const image = data.state === "ok" ? data.data : null;
  const apply = (e: SyntheticEvent) => {
    e.preventDefault();
    const c = Number(center);
    const w = Number(width);
    if (center.trim() !== "" && width.trim() !== "" && Number.isFinite(c) && Number.isFinite(w) && w >= 1) setWin({ center: c, width: w });
  };
  return (
    <Card label={strings.dicomTitle(name)}>
      <SectionHead title={strings.dicomTitle(name)} />
      {data.state === "error" ? <NoticeLine notice={{ kind: "error", text: data.message }} /> : null}
      {image !== null ? (
        <form onSubmit={apply} aria-label={strings.dicomApply} className="mt-3 flex flex-wrap items-end gap-3">
          {image.frames > 1 ? (
            <Labelled label={strings.dicomFrame}>
              {(fid) => (
                <input
                  id={fid}
                  type="number"
                  className={field}
                  min={1}
                  max={image.frames}
                  value={frame + 1}
                  onChange={(e) => {
                    const f = Number(e.target.value) - 1;
                    if (Number.isInteger(f) && f >= 0 && f < image.frames) setFrame(f);
                  }}
                />
              )}
            </Labelled>
          ) : null}
          {image.channels === 1 ? (
            <>
              <Labelled label={strings.dicomPreset}>
                {(pid) => (
                  <select
                    id={pid}
                    className={field}
                    value={win === null ? "" : JSON.stringify(win)}
                    onChange={(e) => {
                      setWin(e.target.value === "" ? null : (JSON.parse(e.target.value) as DicomWindow));
                    }}
                  >
                    <option value="">{strings.dicomPresetAuto}</option>
                    {image.presets.map((p) => (
                      <option key={JSON.stringify(p)} value={JSON.stringify(p)}>
                        {strings.dicomWindow(p.center ?? 0, p.width ?? 0)}
                      </option>
                    ))}
                  </select>
                )}
              </Labelled>
              <Labelled label={strings.dicomCenter}>
                {(cid) => (
                  <input
                    id={cid}
                    type="number"
                    inputMode="decimal"
                    className={field}
                    value={center}
                    onChange={(e) => {
                      setCenter(e.target.value);
                    }}
                  />
                )}
              </Labelled>
              <Labelled label={strings.dicomWidth}>
                {(wid) => (
                  <input
                    id={wid}
                    type="number"
                    inputMode="decimal"
                    min={1}
                    className={field}
                    value={width}
                    onChange={(e) => {
                      setWidth(e.target.value);
                    }}
                  />
                )}
              </Labelled>
              <button type="submit" className="btn btn-ink">
                {strings.dicomApply}
              </button>
            </>
          ) : null}
        </form>
      ) : null}
      {image !== null ? <DicomCanvas image={image} /> : null}
      <button type="button" className="btn btn-soft mt-3" onClick={onClose}>
        {strings.viewerClose}
      </button>
    </Card>
  );
}
