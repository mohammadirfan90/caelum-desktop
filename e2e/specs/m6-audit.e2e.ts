// M6 gate, second part (F-SYS-03): after an audit entry is changed on disk, the Audit screen
// shows the chain as broken. Run after m6.e2e.ts on the same data directory, with the app
// restarted under CAELUM_TE_FAULT=audit-tamper (test equipment only, CR-012 §8).
describe("M6: a tampered audit log is shown as broken", () => {
  before(async () => {
    process.env.CAELUM_TE_FAULT = "audit-tamper";
    await browser.reloadSession();
  });

  it("shows the broken chain and where", async () => {
    await (await $("form[aria-label='Sign in']")).waitForExist({ timeout: 60_000 });
    const label = await $("label=Sign-in name");
    await (await $(`#${CSS.escape(await label.getAttribute("for"))}`)).setValue("cmo.b");
    const secret = await $("label=PIN or password");
    await (await $(`#${CSS.escape(await secret.getAttribute("for"))}`)).setValue("135792");
    await (await $("button=Sign in")).click();
    await (await $("nav[aria-label='Screens'] button=Audit")).click();
    const alert = await $("[role=alert]*=Audit chain BROKEN");
    await alert.waitForExist({ timeout: 20_000 });
    expect(await (await $("*=content changed")).isExisting()).toBe(true);
  });
});
