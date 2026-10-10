// M6 gate (docs/10_BUILD_PLAN.md): pairing → live values → induced desaturation → alarm →
// escalation visible → acknowledge stops escalation (F-ACQ-01, F-ACQ-09, F-ALM-04, F-ALM-05,
// SR-015). The audit-tamper part runs in m6-audit.e2e.ts on a restarted app.
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

const repo = resolve(import.meta.dirname, "..", "..");
const fixtures = resolve(repo, "crates", "caelum-core", "tests", "fixtures", "roster");
const ADMIN = "ground station passphrase";

async function byLabel(text: string) {
  const label = await $(`//label[normalize-space()="${text}"]`);
  const id = await label.getAttribute("for");
  return $(`#${CSS.escape(id)}`);
}

async function signIn(name: string, secret: string) {
  if (await (await $("//button[normalize-space()='Sign out']")).isExisting()) {
    await (await $("//button[normalize-space()='Sign out']")).click();
  }
  await (await byLabel("Sign-in name")).setValue(name);
  await (await byLabel("PIN or password")).setValue(secret);
  await (await $("//button[normalize-space()='Sign in']")).click();
  await (await $("nav[aria-label='Screens']")).waitForExist({ timeout: 20_000 });
}

async function confirmIfAsked(secret: string) {
  const prompt = await $("form[aria-label='Confirm it is you']");
  if (await prompt.isExisting()) {
    await (await prompt.$("input[type=password]")).setValue(secret);
    await (await prompt.$("button[type=submit]")).click();
  }
}

async function screen(name: string) {
  await (await $(`nav[aria-label='Screens'] button=${name}`)).click();
}

describe("M6: from pairing to an acknowledged alarm", () => {
  it("sets up the station: administrator, signed roster, accounts", async () => {
    await (await $("form[aria-label='Create the administrator account']")).waitForExist({ timeout: 60_000 });
    await (await byLabel("Sign-in name")).setValue("admin");
    await (await byLabel("PIN or password")).setValue(ADMIN);
    await (await $("button=Create administrator")).click();
    await signIn("admin", ADMIN);
    await screen("Settings");
    await (await byLabel("Roster file")).setValue(resolve(fixtures, "crew-v1.toml"));
    await (await byLabel("Signature file")).setValue(resolve(fixtures, "crew-v1.sig.toml"));
    await (await $("button=Apply roster")).click();
    await confirmIfAsked(ADMIN);
    await (await $("*=Roster 1 applied")).waitForExist({ timeout: 20_000 });
    for (const [name, role, crew, secret] of [
      ["astro.a", "astronaut", "Fixture Astronaut A", "246813"],
      ["cmo.b", "cmo", "Fixture Officer B", "135792"],
    ] as const) {
      const form = await $("form[aria-label='New account']");
      await (await form.$(`#${CSS.escape(await (await form.$("label=Sign-in name")).getAttribute("for"))}`)).setValue(name);
      await (await form.$("select")).selectByAttribute("value", role);
      await (await form.$("label=Crew member")).waitForExist();
      await (await form.$$("select")[1]).selectByVisibleText(crew);
      await (await form.$("input[type=password]")).setValue(secret);
      await (await form.$("button[type=submit]")).click();
      await confirmIfAsked(ADMIN);
      await (await $(`*=Account ${name} created`)).waitForExist({ timeout: 20_000 });
    }
    // The roster fixture is the signed test file (not clinical content).
    expect(readFileSync(resolve(fixtures, "crew-v1.toml"), "utf8")).toContain("Test fixture");
  });

  it("pairs the emulated oximeter and assigns it to astronaut A", async () => {
    await signIn("cmo.b", "135792");
    await screen("Devices");
    await (await $("button=Scan for devices")).click();
    await (await $("button[aria-label='Pair Test equipment oximeter']")).click();
    await (await $("*=paired. Assign it to a crew member.")).waitForExist({ timeout: 30_000 });
    const select = await $("select[id] option=Fixture Astronaut A").parentElement();
    await select.selectByVisibleText("Fixture Astronaut A");
    await (await $("button[aria-label^='Assign ']")).click();
    await confirmIfAsked("135792");
    await (await $("*=assigned to Fixture Astronaut A")).waitForExist({ timeout: 20_000 });
  });

  it("shows live values, then the desaturation alarm with its escalation", async () => {
    await screen("Home");
    await (await $("select")).selectByVisibleText("Fixture Astronaut A");
    // Live SpO2 from the scenario's first 20 s (no value is ever made up).
    await (await $("*=97")).waitForExist({ timeout: 60_000 });
    const bar = await $("section[aria-label='Active alarms']");
    await bar.$("*=Critical:").waitForExist({ timeout: 120_000 });
    // Escalation stages are shown as they are delivered (astronaut, medical officer, sound…).
    await bar.$("*=Medical officer").waitForExist({ timeout: 30_000 });
  });

  it("acknowledging stops the escalation", async () => {
    const bar = await $("section[aria-label='Active alarms']");
    await (await bar.$("button=Acknowledge")).click();
    await browser.waitUntil(async () => !(await (await bar.$("button=Acknowledge")).isExisting()), { timeout: 20_000 });
    await screen("Alerts");
    await (await $("button^=Show")).click();
    const detail = await $("section[aria-labelledby='detail-title']");
    const stagesBefore = (await detail.$$("ol li")).length;
    await browser.pause(35_000);
    await (await $("button^=Show")).click();
    expect((await (await $("section[aria-labelledby='detail-title']")).$$("ol li")).length).toBe(stagesBefore);
  });
});
