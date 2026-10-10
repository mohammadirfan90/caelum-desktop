import type { RoleName } from "./bindings";

/** Screens of the nav rail (docs/07 §3). */
export const SCREENS = ["home", "body", "alerts", "checks", "env", "medical", "devices", "ground", "settings"] as const;
export type Screen = (typeof SCREENS)[number];

/** ROLE_MATRIX §6: which screens each role sees (tested against the document). */
export const NAVIGATION: Readonly<Record<Exclude<RoleName, "device_gateway">, readonly Screen[]>> = {
  astronaut: ["home", "body", "alerts", "checks", "env", "devices", "settings"],
  cmo: ["home", "body", "alerts", "checks", "env", "medical", "devices", "settings"],
  flight_surgeon: ["home", "body", "alerts", "env", "medical", "devices", "ground", "settings"],
  bme: ["home", "body", "alerts", "env", "devices", "ground", "settings"],
  mcc: ["alerts", "env", "ground", "settings"],
  administrator: ["devices", "ground", "settings"],
};

export function screensFor(role: RoleName): readonly Screen[] {
  return role === "device_gateway" ? [] : NAVIGATION[role];
}
