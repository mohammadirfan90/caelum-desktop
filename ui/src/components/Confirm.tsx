import { type SyntheticEvent, useCallback, useId, useState } from "react";

import type { Api, Res } from "../api";
import type { CommandError } from "../bindings";
import { TechnicalGlyph } from "../RulesetBanner";
import { errorText } from "../screens/Access";
import { strings } from "../strings";

/** Runner type of [`useConfirmed`]. */
export type Run = ReturnType<typeof useConfirmed>["run"];

export type Notice = { readonly kind: "ok" | "error"; readonly text: string } | null;

export function NoticeLine({ notice }: { readonly notice: Notice }) {
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
export function useConfirmed(reauth: Api["authReauth"]) {
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

