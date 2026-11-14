// Settings: account and password, push notifications on this device, API keys (owner/admin), about.
import { useState } from "react";
import type { FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Bell, BellOff, Copy, ExternalLink, KeyRound, Plus, Send, Smartphone } from "lucide-react";
import { Link } from "react-router";
import { api, ApiError, errorText, unwrap, type Schemas } from "../api/client";
import {
  deviceName,
  permission,
  PushDenied,
  pushSupport,
  rememberDevice,
  subscribe,
  thisDevice,
  unsubscribe,
} from "../push";
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
type Device = Schemas["PushSubscriptionView"];
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
      <PageHead
        title="Settings"
        description="Your account, push notifications, API keys and the API reference."
      />
      <div className="stack" style={{ gap: 24 }}>
        <AccountCard />
        <PushCard />
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

const SUPPORT_HINTS = {
  insecure:
    "Push needs HTTPS. Open hetzner-monitor through its https:// address (for example via Tailscale Serve or a reverse proxy) to turn it on for this device.",
  "ios-not-installed":
    "On iPhone and iPad, push works only for apps on the home screen (iOS 16.4 or later): tap Share, then Add to Home Screen, open hetzner-monitor from there and come back to this page.",
  unsupported: "This browser does not support push notifications.",
} as const;

function PushCard() {
  const queryClient = useQueryClient();
  const notify = useToast();
  const [support] = useState(pushSupport);
  const [perm, setPerm] = useState(permission);
  const [current, setCurrent] = useState(thisDevice);
  const [removing, setRemoving] = useState<Device | null>(null);
  const config = useQuery({
    queryKey: ["push-config"],
    queryFn: () => unwrap(api.GET("/api/v1/push/config")),
  });
  const enabled = config.data?.enabled === true;
  const devices = useQuery({
    queryKey: ["push-subscriptions"],
    queryFn: () => unwrap(api.GET("/api/v1/push/subscriptions")),
    enabled,
  });
  const channels = useQuery({
    queryKey: ["channels"],
    queryFn: () => unwrap(api.GET("/api/v1/channels")),
    enabled,
  });
  const list = devices.data?.subscriptions ?? [];
  const thisOne = list.find((d) => d.id === current) ?? null;
  const refresh = () => queryClient.invalidateQueries({ queryKey: ["push-subscriptions"] });

  const enable = useMutation({
    mutationFn: async () => {
      const key = config.data?.vapid_public_key;
      if (!key) throw new Error("The server did not send its push key.");
      const sub = await subscribe(key);
      return unwrap(
        api.POST("/api/v1/push/subscriptions", { body: { ...sub, name: deviceName() } }),
      );
    },
    onSuccess: ({ subscription }) => {
      rememberDevice(subscription.id);
      setCurrent(subscription.id);
      setPerm(permission());
      void refresh();
      notify("success", "Push turned on for this device", subscription.name);
    },
    onError: (err) => {
      setPerm(permission());
      notify(
        "danger",
        "Could not turn on push",
        err instanceof PushDenied
          ? "Notifications are blocked. Allow them in the browser's site settings and try again."
          : errorText(err),
      );
    },
  });

  const disable = useMutation({
    mutationFn: async () => {
      if (thisOne) {
        await unwrap(
          api.DELETE("/api/v1/push/subscriptions/{id}", { params: { path: { id: thisOne.id } } }),
        );
      }
      await unsubscribe();
    },
    onSuccess: () => {
      rememberDevice(null);
      setCurrent(null);
      void refresh();
      notify("success", "Push turned off for this device");
    },
    onError: (err) => notify("danger", "Could not turn off push", errorText(err)),
  });

  const test = useMutation({
    mutationFn: (device: Device) =>
      unwrap(
        api.POST("/api/v1/push/subscriptions/{id}/test", {
          params: { path: { id: device.id } },
        }),
      ),
    onSuccess: (result, device) => {
      if (result.ok) {
        notify("success", "Test push sent", `${device.name} should show it within seconds.`);
        return;
      }
      if (result.gone) void refresh();
      notify(
        "danger",
        "Test push failed",
        result.gone
          ? `The push service no longer knows ${device.name}; it was removed. Turn push on again on that device.`
          : (result.error ?? "Unknown error"),
      );
    },
    onError: (err) => notify("danger", "Test push failed", errorText(err)),
  });

  let thisDeviceBody;
  if (support !== "supported") {
    thisDeviceBody = <p className="muted small">{SUPPORT_HINTS[support]}</p>;
  } else if (thisOne) {
    thisDeviceBody = (
      <div className="row" style={{ flexWrap: "wrap" }}>
        <Badge tone="success">On</Badge>
        <span className="muted small">Alerts arrive here as notifications.</span>
        <Button
          size="sm"
          variant="outline"
          icon={<Send size={16} strokeWidth={1.5} />}
          disabled={test.isPending}
          onClick={() => test.mutate(thisOne)}
        >
          Send test push
        </Button>
        <Button
          size="sm"
          variant="ghost"
          icon={<BellOff size={16} strokeWidth={1.5} />}
          disabled={disable.isPending}
          onClick={() => disable.mutate()}
        >
          Turn off
        </Button>
      </div>
    );
  } else {
    thisDeviceBody = (
      <div className="stack" style={{ gap: 8 }}>
        {perm === "denied" && (
          <p className="muted small">
            Notifications are blocked for this site. Allow them in the browser&apos;s site settings,
            then turn push on.
          </p>
        )}
        <div className="row">
          <Button
            variant="outline"
            icon={<Bell size={16} strokeWidth={1.5} />}
            disabled={enable.isPending || perm === "denied"}
            onClick={() => enable.mutate()}
          >
            {enable.isPending ? "Turning on…" : "Turn on push for this device"}
          </Button>
        </div>
      </div>
    );
  }

  const noChannel =
    channels.data !== undefined && !channels.data.channels.some((c) => c.type === "web_push");

  return (
    <Card title="Push notifications">
      {config.isPending ? (
        <Spinner label="Loading push settings" />
      ) : config.isError ? (
        <ErrorNote>Could not load push settings: {errorText(config.error)}</ErrorNote>
      ) : !enabled ? (
        <p className="muted">
          Web Push is not set up on this server. It needs <code>HM_VAPID_SUBJECT</code> or an{" "}
          <code>https://</code> <code>HM_BASE_URL</code>; see the README.
        </p>
      ) : (
        <div className="stack">
          <div className="stack" style={{ gap: 8 }}>
            <h3 className="small strong">This device</h3>
            {thisDeviceBody}
          </div>
          {noChannel && (
            <p className="muted small">
              Alerts reach your devices through a channel of type Web Push.{" "}
              <Link to="/channels">Add one on the Channels page.</Link>
            </p>
          )}
          <div className="stack" style={{ gap: 8 }}>
            <h3 className="small strong">Your devices</h3>
            {devices.isPending ? (
              <Spinner label="Loading devices" />
            ) : devices.isError ? (
              <ErrorNote>Could not load devices: {errorText(devices.error)}</ErrorNote>
            ) : list.length === 0 ? (
              <Empty title="No devices" icon={<Smartphone size={24} strokeWidth={1.5} />} />
            ) : (
              <div className="table-wrap">
                <table className="table">
                  <thead>
                    <tr>
                      <th>Device</th>
                      <th className="hide-mobile">Push service</th>
                      <th className="hide-mobile">Added</th>
                      <th className="actions">
                        <span className="visually-hidden">Actions</span>
                      </th>
                    </tr>
                  </thead>
                  <tbody>
                    {list.map((d) => (
                      <tr key={d.id}>
                        <td className="strong">
                          {d.name} {d.id === current && <Badge tone="aqua">This device</Badge>}
                        </td>
                        <td className="hide-mobile mono">{d.host}</td>
                        <td className="hide-mobile">
                          <span title={dateTime(d.created_at)}>{relTime(d.created_at)}</span>
                        </td>
                        <td className="actions">
                          <div className="row" style={{ gap: 4, justifyContent: "flex-end" }}>
                            <Button
                              size="sm"
                              variant="outline"
                              disabled={test.isPending}
                              onClick={() => test.mutate(d)}
                            >
                              Send test
                            </Button>
                            <Button size="sm" variant="ghost" onClick={() => setRemoving(d)}>
                              Remove
                            </Button>
                          </div>
                        </td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            )}
          </div>
        </div>
      )}
      <Dialog open={removing !== null} onClose={() => setRemoving(null)} title="Remove device?">
        {removing && (
          <RemoveDevice
            device={removing}
            onDone={(removed) => {
              if (removed && removed === current) {
                rememberDevice(null);
                setCurrent(null);
                void unsubscribe().catch(() => {});
              }
              setRemoving(null);
            }}
          />
        )}
      </Dialog>
    </Card>
  );
}

function RemoveDevice({
  device,
  onDone,
}: {
  device: Device;
  onDone: (removed: string | null) => void;
}) {
  const queryClient = useQueryClient();
  const notify = useToast();
  const remove = useMutation({
    mutationFn: () =>
      unwrap(
        api.DELETE("/api/v1/push/subscriptions/{id}", { params: { path: { id: device.id } } }),
      ),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["push-subscriptions"] });
      notify("success", "Device removed", device.name);
      onDone(device.id);
    },
  });
  return (
    <div className="stack">
      <p className="muted">
        <strong>{device.name}</strong> gets no more push notifications. Turn push on there again to
        add it back.
      </p>
      {remove.isError && <ErrorNote>{errorText(remove.error)}</ErrorNote>}
      <div className="dialog-actions">
        <Button variant="ghost" onClick={() => onDone(null)}>
          Cancel
        </Button>
        <Button variant="danger" disabled={remove.isPending} onClick={() => remove.mutate()}>
          Remove device
        </Button>
      </div>
    </div>
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
