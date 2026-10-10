import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { commands, events } from "./bindings";
import { CrewStation } from "./screens/CrewStation";
import { WaveformWindow } from "./screens/Windows";
import "./styles/index.css";

const root = document.getElementById("root");
if (root === null) {
  throw new Error("index.html is missing the #root element");
}

// F-SYS-13: the second window (waveform viewer) loads the same bundle with `?view=waveform`.
const waveform = new URLSearchParams(window.location.search).get("view") === "waveform";
const dataEvents = {
  onObservations: (handler: (crewId: string) => void) =>
    events.observationsChanged.listen((e) => {
      handler(e.payload.crewId);
    }),
  onDevices: (handler: () => void) =>
    events.devicesChanged.listen(() => {
      handler();
    }),
};

// F-SYS-05: an installation configured as a crew station shows what the primary streams.
const onCrewLan = (handler: () => void) => {
  let stop: (() => void) | null = null;
  void events.crewLanChanged.listen(handler).then((u) => {
    stop = u;
  });
  return () => {
    stop?.();
  };
};
const render = (crewStation: boolean) => {
  createRoot(root).render(
    <StrictMode>
      {waveform ? (
        <WaveformWindow api={commands} events={dataEvents} />
      ) : crewStation ? (
        <CrewStation api={commands} onChange={onCrewLan} />
      ) : (
        <App />
      )}
    </StrictMode>,
  );
};
commands.crewLanStatus().then(
  (r) => {
    render(r.status === "ok" && r.data.configured);
  },
  () => {
    render(false);
  },
);
