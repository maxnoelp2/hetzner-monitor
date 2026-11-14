// Settings: account and password, API keys (owner/admin), about.
import { useState } from "react";
import type { FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Copy, ExternalLink, KeyRound, Plus } from "lucide-react";
import { api, ApiError, errorText, unwrap, type Schemas } from "../api/client";
import { useProfile } from "../api/session";
import { dateTime, relTime } from "../format";
import {
  Badge,
  Button,
  Card,
  Dialog,
  Empty,
  ErrorNote,
  PageHead,
  Spinner,
  TextField,
} from "../ui/components";
import { useToast } from "../ui/toast";

type ApiKey = Schemas["ApiKeyView"];
type Scope = Schemas["Scope"];

const SCOPES: { value: Scope; label: string }[] = [
  { value: "read", label: "read" },
  { value: "write", label: "write" },
  { value: "admin", label: "admin" },
];

const ROLE_LABELS: Record<Schemas["Role"], string> = {
  owner: "Owner",
  admin: "Admin",
  member: "Member",
};

export function SettingsPage() {
  const profile = useProfile();
  const managesKeys = profile.role === "owner" || profile.role === "admin";
  return (
    <>
      <PageHead title="Settings" description="Your account, API keys and the API reference." />
      <div className="stack" style={{ gap: 24 }}>
        <AccountCard />
        {managesKeys ? (
          <ApiKeysCard />
        ) : (
          <Card title="API keys">
            <p className="muted">Only owners and admins of the org manage API keys.</p>
          </Card>
        )}
        <Card title="About">
          <p className="muted">
            Every page here uses the public HTTP API. Its OpenAPI 3.1 document describes all
            endpoints and is the reference for scripts and integrations.
          </p>
          <p>
            <a href="/api/v1/openapi.json" target="_blank" rel="noreferrer">
              OpenAPI document <ExternalLink size={14} strokeWidth={1.5} aria-hidden="true" />
            </a>
          </p>
        </Card>
      </div>
    </>
  );
}

function AccountCard() {
  const profile = useProfile();
  const notify = useToast();
  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [repeat, setRepeat] = useState("");
  const [errors, setErrors] = useState<{ current?: string; next?: string; repeat?: string }>({});
  const [failure, setFailure] = useState<string | null>(null);

  // The variables hold both passwords in plaintext: never keep the mutation in the cache
  // (gcTime 0) and drop it from the observer once it settled (reset in submit).
  const change = useMutation({
    gcTime: 0,
    mutationFn: (body: Schemas["ChangePassword"]) =>
      unwrap(api.PUT("/api/v1/me/password", { body })),
    onSuccess: () => {
      setCurrent("");
      setNext("");
      setRepeat("");
      notify("success", "Password changed. Other sessions were logged out.");
    },
    onError: (e) => {
      if (e instanceof ApiError && e.code === "wrong_password") {
        setErrors({ current: "The current password is wrong." });
      } else {
        setFailure(errorText(e));
      }
    },
  });

  function submit(e: FormEvent) {
    e.preventDefault();
    setFailure(null);
    const found: typeof errors = {};
    if (!current) found.current = "Enter your current password.";
    if (next.length < 12) found.next = "Use at least 12 characters.";
    else if (next.length > 1024) found.next = "Use at most 1024 characters.";
    if (repeat !== next) found.repeat = "The passwords do not match.";
    setErrors(found);
    if (Object.keys(found).length > 0) return;
    change.mutate(
      { current_password: current, new_password: next },
      { onSettled: () => change.reset() },
    );
  }

  return (
    <Card title="Account">
      <dl className="kv">
        <dt>Name</dt>
        <dd>{profile.user.name || "—"}</dd>
        <dt>E-mail</dt>
        <dd>{profile.user.email}</dd>
        <dt>Org</dt>
        <dd>{profile.org.name}</dd>
        <dt>Role</dt>
        <dd>
          <Badge tone="neutral">{ROLE_LABELS[profile.role]}</Badge>
        </dd>
      </dl>
      <form className="form" onSubmit={submit} noValidate>
        <h3 className="card-title" style={{ fontSize: "inherit" }}>
          Change password
        </h3>
        <TextField
          label="Current password"
          type="password"
          autoComplete="current-password"
          value={current}
          error={errors.current}
          onChange={(e) => setCurrent(e.target.value)}
        />
        <div className="form-row">
          <TextField
            label="New password"
            type="password"
            autoComplete="new-password"
            hint="12 to 1024 characters."
            value={next}
            error={errors.next}
            onChange={(e) => setNext(e.target.value)}
          />
          <TextField
            label="Repeat new password"
            type="password"
            autoComplete="new-password"
            value={repeat}
            error={errors.repeat}
            onChange={(e) => setRepeat(e.target.value)}
          />
        </div>
        <p className="small muted">Changing the password logs out your other sessions.</p>
        {failure && <ErrorNote>{failure}</ErrorNote>}
        <div className="row">
          <Button type="submit" variant="secondary" disabled={change.isPending}>
            {change.isPending ? "Changing…" : "Change password"}
          </Button>
        </div>
      </form>
    </Card>
  );
}

function ApiKeysCard() {
  const [creating, setCreating] = useState(false);
  const [revoking, setRevoking] = useState<ApiKey | null>(null);
  const keys = useQuery({
    queryKey: ["api-keys"],
    queryFn: () => unwrap(api.GET("/api/v1/api-keys")),
  });
  const list = keys.data?.api_keys ?? [];

  return (
    <Card
      title="API keys"
      actions={
        <Button
          variant="primary"
          icon={<Plus size={16} strokeWidth={1.5} />}
          onClick={() => setCreating(true)}
        >
          Create API key
        </Button>
      }
    >
      <p className="muted small">
        Keys authenticate scripts and integrations with <code>Authorization: Bearer hm_…</code>.
      </p>
      {keys.isPending ? (
        <Spinner label="Loading API keys" />
      ) : keys.isError ? (
        <ErrorNote>Could not load API keys: {errorText(keys.error)}</ErrorNote>
      ) : list.length === 0 ? (
        <Empty title="No API keys" icon={<KeyRound size={24} strokeWidth={1.5} />} />
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th className="hide-mobile">Key</th>
                <th>Scopes</th>
                <th className="hide-mobile">Created</th>
                <th className="hide-mobile">Last used</th>
                <th className="actions">
                  <span className="visually-hidden">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {list.map((k) => (
                <tr key={k.id}>
                  <td className="strong">{k.name}</td>
                  <td className="hide-mobile mono">hm_{k.prefix}_…</td>
                  <td>
                    <div className="row" style={{ gap: 4 }}>
                      {k.scopes.map((s) => (
                        <Badge key={s} tone={s === "admin" ? "pink" : "aqua"}>
                          {s}
                        </Badge>
                      ))}
                    </div>
                  </td>
                  <td className="hide-mobile">
                    <span title={dateTime(k.created_at)}>{relTime(k.created_at)}</span>
                  </td>
                  <td className="hide-mobile">
                    {k.last_used_at ? (
                      <span title={dateTime(k.last_used_at)}>{relTime(k.last_used_at)}</span>
                    ) : (
                      <span className="subtle">Never</span>
                    )}
                  </td>
                  <td className="actions">
                    {k.revoked_at ? (
                      <span title={dateTime(k.revoked_at)}>
                        <Badge tone="danger">Revoked</Badge>
                      </span>
                    ) : (
                      <Button size="sm" variant="outline" onClick={() => setRevoking(k)}>
                        Revoke
                      </Button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      {/* The dialog renders its body only while open, so the new key is dropped on close. */}
      <Dialog open={creating} onClose={() => setCreating(false)} title="Create API key">
        <CreateKeyBody onDone={() => setCreating(false)} />
      </Dialog>

      <Dialog open={revoking !== null} onClose={() => setRevoking(null)} title="Revoke API key?">
        {revoking && <RevokeKey apiKey={revoking} onDone={() => setRevoking(null)} />}
      </Dialog>
    </Card>
  );
}

function CreateKeyBody({ onDone }: { onDone: () => void }) {
  const queryClient = useQueryClient();
  const notify = useToast();
  const [name, setName] = useState("");
  const [scopes, setScopes] = useState<Scope[]>(["read"]);
  const [errors, setErrors] = useState<{ name?: string; scopes?: string }>({});
  const [key, setKey] = useState<string | null>(null);

  // The response carries the full key: keep it only in this component's state, never in the
  // mutation cache (gcTime 0, reset right after success).
  const create = useMutation({
    gcTime: 0,
    mutationFn: (body: Schemas["CreateKey"]) => unwrap(api.POST("/api/v1/api-keys", { body })),
    onSuccess: (data) => {
      setKey(data.key);
      void queryClient.invalidateQueries({ queryKey: ["api-keys"] });
    },
  });

  function toggle(scope: Scope, on: boolean) {
    setScopes((all) =>
      on
        ? SCOPES.map((s) => s.value).filter((s) => s === scope || all.includes(s))
        : all.filter((s) => s !== scope),
    );
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    const found: typeof errors = {};
    const trimmed = name.trim();
    if (!trimmed) found.name = "Enter a name.";
    else if (trimmed.length > 100) found.name = "Use at most 100 characters.";
    if (scopes.length === 0) found.scopes = "Pick at least one scope.";
    setErrors(found);
    if (Object.keys(found).length > 0) return;
    create.mutate({ name: trimmed, scopes }, { onSuccess: () => create.reset() });
  }

  async function copy() {
    if (!key) return;
    try {
      await navigator.clipboard.writeText(key);
      notify("success", "Key copied");
    } catch {
      notify("danger", "Could not copy", "Select the key and copy it by hand.");
    }
  }

  if (key) {
    return (
      <div className="stack">
        <div className="callout callout-warning">
          <div>This key is shown only once. Store it now, for example in a password manager.</div>
        </div>
        <div className="secret-box" aria-label="New API key">
          {key}
        </div>
        <div className="dialog-actions">
          <Button icon={<Copy size={16} strokeWidth={1.5} />} onClick={() => void copy()}>
            Copy
          </Button>
          <Button variant="secondary" onClick={onDone}>
            Done
          </Button>
        </div>
      </div>
    );
  }

  return (
    <form className="form" onSubmit={submit} noValidate>
      <TextField
        label="Name"
        placeholder="Grafana"
        maxLength={100}
        value={name}
        error={errors.name}
        onChange={(e) => setName(e.target.value)}
      />
      <fieldset className="field" style={{ border: 0, padding: 0, margin: 0 }}>
        <legend className="label">Scopes</legend>
        <div className="checks">
          {SCOPES.map((s) => (
            <label key={s.value} className="check">
              <input
                type="checkbox"
                checked={scopes.includes(s.value)}
                onChange={(e) => toggle(s.value, e.target.checked)}
              />
              {s.label}
            </label>
          ))}
        </div>
        {errors.scopes ? (
          <span className="error">{errors.scopes}</span>
        ) : (
          <span className="hint">
            admin includes write, write includes read. read can only look; write also changes
            projects, rules, channels and silences; admin also manages API keys.
          </span>
        )}
      </fieldset>
      {create.isError && <ErrorNote>{errorText(create.error)}</ErrorNote>}
      <div className="dialog-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={create.isPending}>
          {create.isPending ? "Creating…" : "Create key"}
        </Button>
      </div>
    </form>
  );
}

function RevokeKey({ apiKey, onDone }: { apiKey: ApiKey; onDone: () => void }) {
  const queryClient = useQueryClient();
  const notify = useToast();
  const revoke = useMutation({
    mutationFn: () =>
      unwrap(api.DELETE("/api/v1/api-keys/{id}", { params: { path: { id: apiKey.id } } })),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["api-keys"] });
      notify("success", "API key revoked");
      onDone();
    },
  });
  return (
    <div className="stack">
      <p className="muted">
        Clients that use <strong>{apiKey.name}</strong> stop working right away. This cannot be
        undone.
      </p>
      {revoke.isError && <ErrorNote>{errorText(revoke.error)}</ErrorNote>}
      <div className="dialog-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button variant="danger" disabled={revoke.isPending} onClick={() => revoke.mutate()}>
          Revoke key
        </Button>
      </div>
    </div>
  );
}
