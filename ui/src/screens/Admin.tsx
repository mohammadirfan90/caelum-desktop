import { type SyntheticEvent, useCallback, useEffect, useId, useState } from "react";

import type { Api, Res } from "../api";
import type { CommandError, CrewMember, RoleName, UserItem } from "../bindings";
import { TechnicalGlyph } from "../RulesetBanner";
import { strings } from "../strings";
import { errorText } from "./Access";

/** Roles an administrator can give (machine identities are enrolled with keys, CR-012 §3). */
const ROLES: readonly RoleName[] = ["astronaut", "cmo", "flight_surgeon", "bme", "mcc", "administrator"];
const CREW_ROLES: readonly RoleName[] = ["astronaut", "cmo"];

type Notice = { readonly kind: "ok" | "error"; readonly text: string } | null;

function NoticeLine({ notice }: { readonly notice: Notice }) {
  if (notice === null) return null;
  return notice.kind === "error" ? (
    <p role="alert" className="flex items-center gap-2 text-sm font-semibold">
      <TechnicalGlyph />
      {notice.text}
    </p>
  ) : (
    <p role="status" className="text-sm font-semibold">
      {notice.text}
    </p>
  );
}

/**
 * Runs administrative changes. When the core asks for a recent re-authentication
 * (ROLE_MATRIX §4), it shows the confirm step and then repeats the change.
 */
function useConfirmed(reauth: Api["authReauth"]) {
  const [retry, setRetry] = useState<(() => Promise<void>) | null>(null);
  const run = useCallback(
    async <T,>(action: () => Promise<Res<T>>, onOk: (data: T) => void, onError: (e: CommandError) => void) => {
      const attempt = async (): Promise<void> => {
        const r = await action();
        if (r.status === "ok") onOk(r.data);
        else if (r.error.kind === "reauthRequired") setRetry(() => attempt);
        else onError(r.error);
      };
      await attempt();
    },
    [],
  );
  const confirm = async (secret: string): Promise<CommandError | null> => {
    const r = await reauth(secret);
    if (r.status === "error") return r.error;
    const again = retry;
    setRetry(null);
    if (again !== null) await again();
    return null;
  };
  return {
    run,
    prompt:
      retry === null ? null : (
        <ReauthPrompt
          onConfirm={confirm}
          onCancel={() => {
            setRetry(null);
          }}
        />
      ),
  };
}

function ReauthPrompt({
  onConfirm,
  onCancel,
}: {
  readonly onConfirm: (secret: string) => Promise<CommandError | null>;
  readonly onCancel: () => void;
}) {
  const secretId = useId();
  const [secret, setSecret] = useState("");
  const [error, setError] = useState<string | null>(null);
  const submit = (e: SyntheticEvent) => {
    e.preventDefault();
    void onConfirm(secret).then((err) => {
      setSecret("");
      setError(err === null ? null : errorText(err));
    });
  };
  return (
    <form onSubmit={submit} aria-label={strings.reauthTitle} className="card flex max-w-[28rem] flex-col gap-3 p-5">
      <h3 className="text-lg font-semibold">{strings.reauthTitle}</h3>
      <p className="text-sm text-ink-3">{strings.reauthHelp}</p>
      <label htmlFor={secretId} className="text-sm font-semibold">
        {strings.secret}
      </label>
      <input
        id={secretId}
        type="password"
        autoComplete="current-password"
        // The confirm step appears because of the user's own action; focus goes to its only field.
        // eslint-disable-next-line jsx-a11y/no-autofocus
        autoFocus
        className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
        value={secret}
        onChange={(e) => {
          setSecret(e.target.value);
        }}
      />
      <NoticeLine notice={error === null ? null : { kind: "error", text: error }} />
      <div className="flex flex-wrap gap-3">
        <button type="submit" className="btn btn-ink">
          {strings.reauthConfirm}
        </button>
        <button type="button" className="btn btn-soft" onClick={onCancel}>
          {strings.reauthCancel}
        </button>
      </div>
    </form>
  );
}

function RosterSection({
  api,
  crew,
  run,
  onChanged,
}: {
  readonly api: Api;
  readonly crew: readonly CrewMember[] | null;
  readonly run: ReturnType<typeof useConfirmed>["run"];
  readonly onChanged: () => void;
}) {
  const fileId = useId();
  const sigId = useId();
  const [bundle, setBundle] = useState<File | null>(null);
  const [signature, setSignature] = useState<File | null>(null);
  const [notice, setNotice] = useState<Notice>(null);
  const apply = (e: SyntheticEvent) => {
    e.preventDefault();
    if (bundle === null || signature === null) {
      setNotice({ kind: "error", text: strings.rosterChooseFiles });
      return;
    }
    Promise.all([bundle.text(), signature.text()]).then(
      ([b, s]) =>
        run(
          () => api.configApplyCrew(b, s),
          (r) => {
            const text = strings.rosterApplied(r.version, r.members);
            setNotice({ kind: "ok", text: r.development ? `${text} ${strings.rosterDevelopment}` : text });
            onChanged();
          },
          (err) => {
            setNotice({ kind: "error", text: errorText(err) });
          },
        ),
      () => {
        setNotice({ kind: "error", text: strings.rosterReadFailed });
      },
    );
  };
  return (
    <section aria-labelledby="roster-title" className="flex flex-col gap-4">
      <h3 id="roster-title" className="text-xl font-semibold">
        {strings.rosterTitle}
      </h3>
      {crew === null || crew.length === 0 ? (
        <p className="text-sm text-ink-3">{strings.rosterEmpty}</p>
      ) : (
        <table className="w-full text-left text-sm">
          <thead>
            <tr>
              <th scope="col">{strings.crewColumns.name}</th>
              <th scope="col">{strings.crewColumns.position}</th>
              <th scope="col">{strings.crewColumns.mission}</th>
              <th scope="col">{strings.crewColumns.status}</th>
            </tr>
          </thead>
          <tbody>
            {crew.map((c) => (
              <tr key={c.id}>
                <td>{c.name}</td>
                <td>{c.position}</td>
                <td>{c.mission}</td>
                <td>{c.active ? strings.crewActive : strings.crewInactive}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
      <form onSubmit={apply} aria-label={strings.rosterApply} className="flex flex-col gap-3">
        <p className="text-sm text-ink-3">{strings.rosterHelp}</p>
        <label htmlFor={fileId} className="text-sm font-semibold">
          {strings.rosterFile}
        </label>
        <input
          id={fileId}
          type="file"
          accept=".toml"
          className="min-h-12"
          onChange={(e) => {
            setBundle(e.target.files?.[0] ?? null);
          }}
        />
        <label htmlFor={sigId} className="text-sm font-semibold">
          {strings.rosterSignature}
        </label>
        <input
          id={sigId}
          type="file"
          accept=".toml"
          className="min-h-12"
          onChange={(e) => {
            setSignature(e.target.files?.[0] ?? null);
          }}
        />
        <NoticeLine notice={notice} />
        <button type="submit" className="btn btn-ink self-start">
          {strings.rosterApply}
        </button>
      </form>
    </section>
  );
}

function UsersSection({
  api,
  self,
  crew,
  users,
  run,
  onChanged,
}: {
  readonly api: Api;
  readonly self: string;
  readonly crew: readonly CrewMember[] | null;
  readonly users: readonly UserItem[] | null;
  readonly run: ReturnType<typeof useConfirmed>["run"];
  readonly onChanged: () => void;
}) {
  const nameId = useId();
  const roleId = useId();
  const crewId = useId();
  const secretId = useId();
  const [username, setUsername] = useState("");
  const [role, setRole] = useState<RoleName>("astronaut");
  const [crewMember, setCrewMember] = useState("");
  const [secret, setSecret] = useState("");
  const [notice, setNotice] = useState<Notice>(null);
  const needsCrew = CREW_ROLES.includes(role);
  const crewName = (id: string | null) => crew?.find((c) => c.id === id)?.name ?? strings.userNoCrew;
  const create = (e: SyntheticEvent) => {
    e.preventDefault();
    const name = username;
    void run(
      () => api.userCreate(name, role, needsCrew && crewMember !== "" ? crewMember : null, secret),
      () => {
        setNotice({ kind: "ok", text: strings.userCreated(name.toLowerCase()) });
        setUsername("");
        setSecret("");
        onChanged();
      },
      (err) => {
        setNotice({ kind: "error", text: errorText(err) });
      },
    );
  };
  const disable = (u: UserItem) => {
    void run(
      () => api.userDisable(u.id),
      () => {
        setNotice({ kind: "ok", text: strings.userDisabledDone(u.username) });
        onChanged();
      },
      (err) => {
        setNotice({ kind: "error", text: errorText(err) });
      },
    );
  };
  return (
    <section aria-labelledby="users-title" className="flex flex-col gap-4">
      <h3 id="users-title" className="text-xl font-semibold">
        {strings.usersTitle}
      </h3>
      <table className="w-full text-left text-sm">
        <thead>
          <tr>
            <th scope="col">{strings.userColumns.username}</th>
            <th scope="col">{strings.userColumns.role}</th>
            <th scope="col">{strings.userColumns.crew}</th>
            <th scope="col">{strings.userColumns.status}</th>
            <th scope="col">{strings.userColumns.action}</th>
          </tr>
        </thead>
        <tbody>
          {(users ?? []).map((u) => (
            <tr key={u.id}>
              <td>{u.username}</td>
              <td>{strings.role[u.role]}</td>
              <td>{crewName(u.crewMemberId)}</td>
              <td>{u.disabled ? strings.userDisabled : strings.userActive}</td>
              <td>
                {u.disabled || u.id === self ? null : (
                  <button
                    type="button"
                    className="btn btn-soft"
                    aria-label={strings.userDisableLabel(u.username)}
                    onClick={() => {
                      disable(u);
                    }}
                  >
                    {strings.userDisable}
                  </button>
                )}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
      <form onSubmit={create} aria-label={strings.newUserTitle} className="card flex max-w-[34rem] flex-col gap-3 p-5">
        <h4 className="text-lg font-semibold">{strings.newUserTitle}</h4>
        <label htmlFor={nameId} className="text-sm font-semibold">
          {strings.username}
        </label>
        <input
          id={nameId}
          autoComplete="off"
          className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
          value={username}
          onChange={(e) => {
            setUsername(e.target.value);
          }}
        />
        <label htmlFor={roleId} className="text-sm font-semibold">
          {strings.newUserRole}
        </label>
        <select
          id={roleId}
          className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
          value={role}
          onChange={(e) => {
            setRole(e.target.value as RoleName);
          }}
        >
          {ROLES.map((r) => (
            <option key={r} value={r}>
              {strings.role[r]}
            </option>
          ))}
        </select>
        {needsCrew ? (
          <>
            <label htmlFor={crewId} className="text-sm font-semibold">
              {strings.newUserCrew}
            </label>
            <select
              id={crewId}
              className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
              value={crewMember}
              onChange={(e) => {
                setCrewMember(e.target.value);
              }}
            >
              <option value="">{strings.newUserCrewChoose}</option>
              {(crew ?? [])
                .filter((c) => c.active)
                .map((c) => (
                  <option key={c.id} value={c.id}>
                    {c.name}
                  </option>
                ))}
            </select>
          </>
        ) : null}
        <label htmlFor={secretId} className="text-sm font-semibold">
          {strings.newUserSecret}
        </label>
        <input
          id={secretId}
          type="password"
          autoComplete="new-password"
          className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
          value={secret}
          onChange={(e) => {
            setSecret(e.target.value);
          }}
        />
        <NoticeLine notice={notice} />
        <button type="submit" className="btn btn-ink self-start">
          {strings.newUserCreate}
        </button>
      </form>
    </section>
  );
}

/** Crew roster and accounts for the administrator (F-SYS-07, CR-012 §2–3, CR-007 §4). */
export function Administration({ api, self }: { readonly api: Api; readonly self: string }) {
  const [crew, setCrew] = useState<CrewMember[] | null>(null);
  const [users, setUsers] = useState<UserItem[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const { run, prompt } = useConfirmed(api.authReauth);
  const reload = useCallback(() => {
    void Promise.all([api.crewList(), api.userList()]).then(([c, u]) => {
      if (c.status === "ok") setCrew(c.data);
      if (u.status === "ok") setUsers(u.data);
      const failed = [c, u].find((r) => r.status === "error");
      setLoadError(failed?.status === "error" ? errorText(failed.error) : null);
    });
  }, [api]);
  useEffect(() => {
    reload();
  }, [reload]);
  return (
    <section aria-labelledby="admin-title" className="flex max-w-[60rem] flex-col gap-8">
      <h2 id="admin-title" className="text-2xl font-semibold">
        {strings.adminTitle}
      </h2>
      <NoticeLine notice={loadError === null ? null : { kind: "error", text: loadError }} />
      {prompt}
      <RosterSection api={api} crew={crew} run={run} onChanged={reload} />
      <UsersSection api={api} self={self} crew={crew} users={users} run={run} onChanged={reload} />
    </section>
  );
}
