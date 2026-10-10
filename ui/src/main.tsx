import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import { App } from "./App";
import { commands, events } from "./bindings";
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

createRoot(root).render(
  <StrictMode>{waveform ? <WaveformWindow api={commands} events={dataEvents} /> : <App />}</StrictMode>,
);
