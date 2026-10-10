// Administration (F-SYS-07, F-SYS-01; CR-012 §2–3, CR-007 §4): signed roster apply, accounts,
// and the re-authentication step the core asks for before every change (ROLE_MATRIX §4).
import { fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import type { Api } from "../src/api";
import type { CrewMember, UserItem } from "../src/bindings";
import { Administration } from "../src/screens/Admin";

const ok = <T,>(data: T) => Promise.resolve({ status: "ok" as const, data });
const err = (kind: "reauthRequired" | "rejected" | "invalidCredentials", message: string = kind) =>
  Promise.resolve({ status: "error" as const, error: { kind, message, retryAfterS: null } });

const crew: CrewMember[] = [{ id: "c1", name: "Fixture Astronaut A", position: "flight engineer", mission: "TEST-1", active: true }];
const users: UserItem[] = [
  { id: "u0", username: "admin", role: "administrator", crewMemberId: null, disabled: false },
  { id: "u1", username: "astro.a", role: "astronaut", crewMemberId: "c1", disabled: false },
];

function station(overrides: Partial<Api> = {}) {
  return {
    crewList: () => ok(crew),
    userList: () => ok(users),
    authReauth: vi.fn(() => ok(null)),
    ...overrides,
  } as unknown as Api;
}

describe("administration", () => {
  it("lists the roster and accounts; never offers to disable your own account", async () => {
    render(<Administration api={station()} self="u0" />);
    const [table] = await screen.findAllByRole("table");
    if (table === undefined) throw new Error("no roster table");
    expect(within(table).getByText("Fixture Astronaut A")).toBeInTheDocument();
    expect(await screen.findByText("astro.a")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Disable account astro.a" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Disable account admin" })).not.toBeInTheDocument();
  });

  it("asks for re-authentication when the core requires it, then repeats the change (ROLE_MATRIX §4)", async () => {
    let confirmed = false;
    const userCreate = vi.fn(() => (confirmed ? ok("u9") : err("reauthRequired")));
    const authReauth = vi.fn((secret: string) => {
      if (secret !== "ground station passphrase") return err("invalidCredentials");
      confirmed = true;
      return ok(null);
    });
    render(<Administration api={station({ userCreate, authReauth })} self="u0" />);
    const form = await screen.findByRole("form", { name: "New account" });
    fireEvent.change(within(form).getByLabelText("Sign-in name"), { target: { value: "Astro.B" } });
    fireEvent.change(within(form).getByLabelText("Crew member"), { target: { value: "c1" } });
    fireEvent.change(within(form).getByLabelText("Initial PIN or password"), { target: { value: "246813" } });
    fireEvent.submit(form);

    const prompt = await screen.findByRole("form", { name: "Confirm it is you" });
    fireEvent.change(within(prompt).getByLabelText("PIN or password"), { target: { value: "wrong" } });
    fireEvent.submit(prompt);
    expect(await within(prompt).findByRole("alert")).toHaveTextContent("Sign-in name or PIN/password is not correct.");

    fireEvent.change(within(prompt).getByLabelText("PIN or password"), { target: { value: "ground station passphrase" } });
    fireEvent.submit(prompt);
    expect(await screen.findByText("Account astro.b created.")).toBeInTheDocument();
    expect(userCreate).toHaveBeenCalledTimes(2);
    expect(userCreate).toHaveBeenLastCalledWith("Astro.B", "astronaut", "c1", "246813");
    expect(screen.queryByRole("form", { name: "Confirm it is you" })).not.toBeInTheDocument();
  });

  it("sends no crew member for ground roles and shows the core's reason when it refuses", async () => {
    const userCreate = vi.fn(() => err("rejected", "this sign-in name is taken"));
    render(<Administration api={station({ userCreate })} self="u0" />);
    const form = await screen.findByRole("form", { name: "New account" });
    fireEvent.change(within(form).getByLabelText("Role"), { target: { value: "bme" } });
    expect(within(form).queryByLabelText("Crew member")).not.toBeInTheDocument();
    fireEvent.change(within(form).getByLabelText("Sign-in name"), { target: { value: "bme.one" } });
    fireEvent.submit(form);
    expect(await within(form).findByRole("alert")).toHaveTextContent("Refused: this sign-in name is taken.");
    expect(userCreate).toHaveBeenCalledWith("bme.one", "bme", null, "");
  });

  it("applies a signed roster from its two files and says when it is development-signed", async () => {
    const configApplyCrew = vi.fn(() => ok({ version: "1", members: 1, development: true }));
    render(<Administration api={station({ configApplyCrew })} self="u0" />);
    const form = await screen.findByRole("form", { name: "Apply roster" });
    fireEvent.submit(form);
    expect(await within(form).findByRole("alert")).toHaveTextContent("Choose the roster file and its signature file.");
    fireEvent.change(within(form).getByLabelText("Roster file"), {
      target: { files: [new File(["[roster]"], "crew.toml")] },
    });
    fireEvent.change(within(form).getByLabelText("Signature file"), {
      target: { files: [new File(["kind = \"crew\""], "crew.sig.toml")] },
    });
    fireEvent.submit(form);
    expect(await within(form).findByRole("status")).toHaveTextContent(
      "Roster 1 applied: 1 crew members on the roster. Signed with development keys: not for operational use.",
    );
    expect(configApplyCrew).toHaveBeenCalledWith("[roster]", 'kind = "crew"');
  });

  it("disables an account and reloads the list", async () => {
    const userDisable = vi.fn(() => ok(null));
    let listed = 0;
    const userList = () => {
      listed += 1;
      return ok(users);
    };
    render(<Administration api={station({ userDisable, userList })} self="u0" />);
    fireEvent.click(await screen.findByRole("button", { name: "Disable account astro.a" }));
    expect(await screen.findByText("Account astro.a disabled.")).toBeInTheDocument();
    expect(userDisable).toHaveBeenCalledWith("u1");
    await waitFor(() => {
      expect(listed).toBe(2);
    });
  });
});
