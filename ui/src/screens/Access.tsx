import { type SyntheticEvent, useId, useState } from "react";

import type { CommandError } from "../bindings";
import { TechnicalGlyph } from "../RulesetBanner";
import { strings } from "../strings";

export type Result = { status: "ok" } | { status: "error"; error: CommandError };

/** User-facing text for a command error (messages are in the catalog). */
export function errorText(e: CommandError): string {
  switch (e.kind) {
    case "throttled":
      return strings.errors.throttled(e.retryAfterS ?? 0);
    case "signInRequired":
    case "locked":
    case "notPermitted":
    case "invalidCredentials":
    case "accountDisabled":
    case "notInteractive":
    case "reauthRequired":
    case "unavailable":
    case "failed":
      return strings.errors[e.kind];
  }
}

function ErrorLine({ error }: { readonly error: string | null }) {
  if (error === null) return null;
  return (
    <p role="alert" className="flex items-center gap-2 text-sm font-semibold">
      <TechnicalGlyph />
      {error}
    </p>
  );
}

function SecretForm({
  title,
  help,
  withUsername,
  submit,
  onSubmit,
}: {
  readonly title: string;
  readonly help?: string;
  readonly withUsername: boolean;
  readonly submit: string;
  readonly onSubmit: (username: string, secret: string) => Promise<Result>;
}) {
  const userId = useId();
  const secretId = useId();
  const [username, setUsername] = useState("");
  const [secret, setSecret] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const send = (e: SyntheticEvent) => {
    e.preventDefault();
    setBusy(true);
    setError(null);
    void onSubmit(username, secret).then((r) => {
      setBusy(false);
      setSecret("");
      if (r.status === "error") setError(errorText(r.error));
    });
  };
  return (
    <form onSubmit={send} className="card flex w-full max-w-[28rem] flex-col gap-4 p-6" aria-label={title}>
      <h2 className="text-xl font-semibold">{title}</h2>
      {help === undefined ? null : <p className="text-sm text-ink-3">{help}</p>}
      {withUsername ? (
        <div className="flex flex-col gap-1">
          <label htmlFor={userId} className="text-sm font-semibold">
            {strings.username}
          </label>
          <input
            id={userId}
            className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
            autoComplete="username"
            value={username}
            onChange={(e) => {
              setUsername(e.target.value);
            }}
          />
        </div>
      ) : null}
      <div className="flex flex-col gap-1">
        <label htmlFor={secretId} className="text-sm font-semibold">
          {strings.secret}
        </label>
        <input
          id={secretId}
          type="password"
          className="min-h-12 rounded-lg border border-ink-3 bg-surface px-3"
          autoComplete="current-password"
          value={secret}
          onChange={(e) => {
            setSecret(e.target.value);
          }}
        />
      </div>
      <ErrorLine error={error} />
      <button type="submit" className="btn btn-ink" disabled={busy}>
        {busy ? strings.signingIn : submit}
      </button>
    </form>
  );
}

/** Sign-in, or first-administrator enrolment on a new station; break-glass always available. */
export function SignIn({
  needsSetup,
  onSignIn,
  onCreateAdministrator,
  onEmergency,
}: {
  readonly needsSetup: boolean;
  readonly onSignIn: (username: string, secret: string) => Promise<Result>;
  readonly onCreateAdministrator: (username: string, secret: string) => Promise<Result>;
  readonly onEmergency: () => void;
}) {
  return (
    <div className="flex flex-col items-center gap-6">
      {needsSetup ? (
        <SecretForm title={strings.setupTitle} help={strings.setupHelp} withUsername submit={strings.setupCreate} onSubmit={onCreateAdministrator} />
      ) : (
        <SecretForm title={strings.signInTitle} withUsername submit={strings.signIn} onSubmit={onSignIn} />
      )}
      <EmergencyButton onEmergency={onEmergency} />
    </div>
  );
}

/** SR-014: the emergency view never needs a sign-in. */
export function EmergencyButton({ onEmergency }: { readonly onEmergency: () => void }) {
  return (
    <button type="button" className="btn btn-lg btn-soft" aria-describedby="emergency-help" onClick={onEmergency}>
      {strings.emergency}
      <span id="emergency-help" className="sr-only">
        {strings.emergencyHelp}
      </span>
    </button>
  );
}

/** A locked session: unlock with the same user's secret, or sign out. Alarms stay on screen. */
export function Locked({
  name,
  onUnlock,
  onSignOut,
  onEmergency,
}: {
  readonly name: string;
  readonly onUnlock: (secret: string) => Promise<Result>;
  readonly onSignOut: () => void;
  readonly onEmergency: () => void;
}) {
  return (
    <div className="flex flex-col items-center gap-6">
      <SecretForm
        title={strings.lockedTitle}
        help={strings.lockedHelp(name)}
        withUsername={false}
        submit={strings.unlock}
        onSubmit={(_u, secret) => onUnlock(secret)}
      />
      <div className="flex gap-3">
        <button type="button" className="btn btn-soft" onClick={onSignOut}>
          {strings.signOut}
        </button>
        <EmergencyButton onEmergency={onEmergency} />
      </div>
    </div>
  );
}
