// SR-009 banner (docs/07 §3): development-signed rulesets, rejected rulesets and the safe state are
// always visible, as text with a glyph; an unreadable status is never shown as fine.
import { render, screen } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import type { RulesetInfo } from "../src/bindings";
import { RulesetBanner } from "../src/RulesetBanner";

const ok = (data: RulesetInfo) => () => Promise.resolve({ status: "ok" as const, data });

describe("ruleset banner", () => {
  it("shows DEVELOPMENT RULESET with the version for a development-signed ruleset", async () => {
    render(<RulesetBanner load={ok({ state: "verified", version: "1.0.0-draft.2", development: true, reason: null })} />);
    const banner = await screen.findByRole("complementary", { name: "Alarm ruleset" });
    expect(banner).toHaveTextContent("DEVELOPMENT RULESET 1.0.0-draft.2: not for operational use");
  });

  it("shows nothing for a verified flight-signed ruleset", async () => {
    const { container } = render(<RulesetBanner load={ok({ state: "verified", version: "1.0.0", development: false, reason: null })} />);
    await Promise.resolve();
    expect(container).toBeEmptyDOMElement();
  });

  it("alerts when the installed ruleset was rejected and says which one is in use", async () => {
    render(<RulesetBanner load={ok({ state: "fallback", version: "1.0.0-draft.1", development: true, reason: "digest mismatch" })} />);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The installed alarm ruleset was rejected; ruleset 1.0.0-draft.1 is in use");
    expect(alert).toHaveTextContent("digest mismatch");
    expect(screen.getByRole("complementary")).toHaveTextContent("DEVELOPMENT RULESET");
  });

  it("alerts that physiological rules are not running in the safe state", async () => {
    render(<RulesetBanner load={ok({ state: "safeState", version: null, development: false, reason: "untrusted key" })} />);
    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("Physiological alarm rules are not running: no verified ruleset");
    expect(alert).toHaveTextContent("untrusted key");
  });

  it("alerts when the status cannot be read, from an error result or a rejection", async () => {
    const failed = () =>
      Promise.resolve({
        status: "error" as const,
        error: { kind: "unavailable" as const, message: "records unavailable", retryAfterS: null },
      });
    const { unmount } = render(<RulesetBanner load={failed} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("Alarm ruleset status unavailable");
    unmount();
    // eslint-disable-next-line @typescript-eslint/prefer-promise-reject-errors
    render(<RulesetBanner load={() => Promise.reject("ruleset_status not allowed")} />);
    expect(await screen.findByRole("alert")).toHaveTextContent("ruleset_status not allowed");
  });
});
