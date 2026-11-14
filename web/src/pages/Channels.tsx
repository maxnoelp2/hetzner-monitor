// Notification channels: list, enable/disable, send a test, add, edit and delete.
// Channel configs are write-only: the API only returns a non-secret `hint`.
import { useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  Bell,
  Copy,
  Hash,
  Mail,
  MessageSquare,
  Pencil,
  Plus,
  RefreshCw,
  Send,
  Smartphone,
  Trash,
  Webhook,
} from "lucide-react";
import { Link } from "react-router";
import type { LucideIcon } from "lucide-react";
import { api, ApiError, errorText, unwrap, type Schemas } from "../api/client";
import { SEVERITIES, severityTone, type Severity } from "../format";
import {
  Badge,
  Button,
  Dialog,
  Empty,
  ErrorNote,
  IconButton,
  PageHead,
  SelectField,
  Spinner,
  Switch,
  TextAreaField,
  TextField,
} from "../ui/components";
import { useToast } from "../ui/toast";

type Channel = Schemas["ChannelView"];
type ChannelType = Schemas["ChannelType"];
type ChannelConfig = Schemas["ChannelConfig"];

const TYPE_INFO: Record<ChannelType, { label: string; icon: LucideIcon }> = {
  email: { label: "Email", icon: Mail },
  slack: { label: "Slack", icon: Hash },
  discord: { label: "Discord", icon: MessageSquare },
  webhook: { label: "Webhook", icon: Webhook },
  ntfy: { label: "ntfy", icon: Bell },
  web_push: { label: "Web Push", icon: Smartphone },
};

const TYPES = (Object.keys(TYPE_INFO) as ChannelType[]).map((type) => ({
  type,
  ...TYPE_INFO[type],
}));

function typeInfo(type: ChannelType) {
  return TYPE_INFO[type];
}

const SEVERITY_LABELS: Record<Severity, string> = {
  info: "Info",
  warning: "Warning",
  critical: "Critical",
};

const MAX_RECIPIENTS = 20;
const TOPIC_RE = /^[A-Za-z0-9_-]{1,64}$/;
const EMAIL_RE = /^[^\s@]+@[^\s@]+$/;

/** Form state of a channel config; holds secrets, so it lives only inside an open dialog. */
type ConfigDraft = {
  to: string;
  url: string;
  secret: string;
  server: string;
  topic: string;
  token: string;
};
const EMPTY_CONFIG: ConfigDraft = { to: "", url: "", secret: "", server: "", topic: "", token: "" };

type FieldErrors = Partial<Record<keyof ConfigDraft | "name", string>>;

/** A random signing secret: 32 bytes, base64url without padding (43 characters). */
function generateSecret(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function isHttpUrl(value: string): boolean {
  try {
    const u = new URL(value);
    return u.protocol === "https:" || u.protocol === "http:";
  } catch {
    return false;
  }
}

/** Validates the draft for `type` and builds the API config, or returns field errors. */
function buildConfig(
  type: ChannelType,
  d: ConfigDraft,
): { config: ChannelConfig; errors?: undefined } | { config?: undefined; errors: FieldErrors } {
  switch (type) {
    case "email": {
      const to = d.to
        .split(/[\s,;]+/)
        .map((s) => s.trim())
        .filter(Boolean);
      if (to.length === 0) return { errors: { to: "Add at least one address." } };
      if (to.length > MAX_RECIPIENTS) {
        return { errors: { to: `At most ${MAX_RECIPIENTS} addresses.` } };
      }
      const bad = to.find((a) => !EMAIL_RE.test(a));
      if (bad) return { errors: { to: `Not an e-mail address: ${bad}` } };
      return { config: { to } };
    }
    case "slack":
    case "discord": {
      const url = d.url.trim();
      if (!isHttpUrl(url)) return { errors: { url: "Enter the full webhook URL." } };
      return { config: { url } };
    }
    case "webhook": {
      const url = d.url.trim();
      const errors: FieldErrors = {};
      if (!isHttpUrl(url)) errors.url = "Enter the full URL of the receiver.";
      if (d.secret.length < 16 || d.secret.length > 256) {
        errors.secret = "Use 16 to 256 characters, or generate one.";
      }
      if (errors.url || errors.secret) return { errors };
      return { config: { url, secret: d.secret } };
    }
    case "ntfy": {
      const server = d.server.trim();
      const topic = d.topic.trim();
      const errors: FieldErrors = {};
      if (server && !isHttpUrl(server)) errors.server = "Enter the full server URL.";
      if (!TOPIC_RE.test(topic)) {
        errors.topic = "Use 1 to 64 letters, digits, _ or -.";
      }
      if (errors.server || errors.topic) return { errors };
      const config: ChannelConfig = { topic };
      if (server) config.server = server;
      if (d.token) config.token = d.token;
      return { config };
    }
    case "web_push":
      // Every member's subscribed devices; nothing to configure here.
      return { config: {} };
  }
}

/** Maps a save error to a field error or a form-level message. */
function saveError(err: unknown): { name?: string; form?: string } {
  if (err instanceof ApiError) {
    if (err.code === "name_taken") return { name: "Another channel already has this name." };
    if (err.code === "push_disabled") {
      return {
        form: "Web Push is not set up on this server. It needs HM_VAPID_SUBJECT or an https:// HM_BASE_URL; see the README.",
      };
    }
    if (err.code === "email_disabled") {
      return {
        form: "Email is not set up on this server. Set HM_SMTP_HOST (and the other HM_SMTP_* settings) on the server and restart it, or pick another channel type.",
      };
    }
  }
  return { form: errorText(err) };
}

// ---- page ----

type DialogState =
  | { kind: "create" }
  | { kind: "edit"; channel: Channel }
  | { kind: "delete"; channel: Channel }
  | null;

export function ChannelsPage() {
  const [dialog, setDialog] = useState<DialogState>(null);
  const close = () => setDialog(null);
  const channels = useQuery({
    queryKey: ["channels"],
    queryFn: () => unwrap(api.GET("/api/v1/channels")),
  });

  let body: ReactNode;
  if (channels.isPending) {
    body = <Spinner label="Loading channels" />;
  } else if (channels.isError) {
    body = <ErrorNote>{errorText(channels.error)}</ErrorNote>;
  } else if (channels.data.channels.length === 0) {
    body = (
      <Empty title="No channels yet" icon={<Send size={24} strokeWidth={1.5} />}>
        <p className="muted small">
          Add a channel to get alerts by email, Slack, Discord, a signed webhook, ntfy or as a push
          notification on your devices.
        </p>
      </Empty>
    );
  } else {
    body = (
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Type</th>
              <th className="hide-mobile">Target</th>
              <th className="hide-mobile">Min severity</th>
              <th>Enabled</th>
              <th>
                <span className="visually-hidden">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {channels.data.channels.map((ch) => (
              <ChannelRow
                key={ch.id}
                channel={ch}
                onEdit={() => setDialog({ kind: "edit", channel: ch })}
                onDelete={() => setDialog({ kind: "delete", channel: ch })}
              />
            ))}
          </tbody>
        </table>
      </div>
    );
  }

  return (
    <div className="stack">
      <PageHead
        title="Channels"
        description="Where alerts are sent. A channel gets every alert at or above its minimum severity, unless a rule is pinned to specific channels."
        actions={
          <Button
            variant="primary"
            icon={<Plus size={16} strokeWidth={1.5} />}
            onClick={() => setDialog({ kind: "create" })}
          >
            Add channel
          </Button>
        }
      />
      {body}

      <Dialog open={dialog?.kind === "create"} onClose={close} title="Add channel">
        {dialog?.kind === "create" && <CreateChannelForm onDone={close} />}
      </Dialog>
      <Dialog open={dialog?.kind === "edit"} onClose={close} title="Edit channel">
        {dialog?.kind === "edit" && <EditChannelForm channel={dialog.channel} onDone={close} />}
      </Dialog>
      <Dialog open={dialog?.kind === "delete"} onClose={close} title="Delete channel">
        {dialog?.kind === "delete" && <DeleteChannel channel={dialog.channel} onDone={close} />}
      </Dialog>
    </div>
  );
}

function TypeLabel({ type }: { type: ChannelType }) {
  const { label, icon: Icon } = typeInfo(type);
  return (
    <span className="row" style={{ gap: 8, flexWrap: "nowrap" }}>
      <Icon size={16} strokeWidth={1.5} aria-hidden="true" />
      {label}
    </span>
  );
}

function ChannelRow({
  channel,
  onEdit,
  onDelete,
}: {
  channel: Channel;
  onEdit: () => void;
  onDelete: () => void;
}) {
  const queryClient = useQueryClient();
  const notify = useToast();

  const toggle = useMutation({
    mutationFn: (enabled: boolean) =>
      unwrap(
        api.PATCH("/api/v1/channels/{id}", {
          params: { path: { id: channel.id } },
          body: { enabled },
        }),
      ),
    onSuccess: ({ channel: updated }) => {
      queryClient.setQueryData<Schemas["ChannelList"]>(["channels"], (old) =>
        old ? { channels: old.channels.map((c) => (c.id === updated.id ? updated : c)) } : old,
      );
      notify("success", updated.enabled ? "Channel enabled" : "Channel disabled", updated.name);
    },
    onError: (err) => notify("danger", "Could not change the channel", errorText(err)),
    onSettled: () => queryClient.invalidateQueries({ queryKey: ["channels"] }),
  });

  const test = useMutation({
    mutationFn: () =>
      unwrap(api.POST("/api/v1/channels/{id}/test", { params: { path: { id: channel.id } } })),
    onSuccess: (result) => {
      const devices = result.devices;
      if (devices && devices.length > 0) {
        const failed = devices.filter((d) => !d.ok);
        const summary = devices
          .map((d) => (d.ok ? `${d.name}: sent` : `${d.name}: ${d.error ?? "failed"}`))
          .join("; ");
        notify(
          failed.length === 0 ? "success" : "danger",
          failed.length === 0 ? "Test sent" : "Test failed",
          summary,
        );
      } else if (result.ok) {
        notify("success", "Test sent", channel.name);
      } else {
        const error = result.error ?? "Unknown error";
        notify(
          "danger",
          "Test failed",
          result.retryable ? `${error} (temporary, try again)` : error,
        );
      }
    },
    onError: (err) => notify("danger", "Test failed", errorText(err)),
  });

  const enabled = toggle.isPending ? toggle.variables : channel.enabled;

  return (
    <tr>
      <td className="strong">{channel.name}</td>
      <td>
        <TypeLabel type={channel.type} />
      </td>
      <td className="hide-mobile mono">{channel.hint}</td>
      <td className="hide-mobile">
        <Badge tone={severityTone(channel.min_severity)} dot>
          {SEVERITY_LABELS[channel.min_severity]}
        </Badge>
      </td>
      <td>
        <Switch
          checked={enabled}
          disabled={toggle.isPending}
          onChange={(v) => toggle.mutate(v)}
          label={<span className="visually-hidden">{`${channel.name} enabled`}</span>}
        />
      </td>
      <td className="actions">
        <Button
          size="sm"
          variant="ghost"
          icon={<Send size={16} strokeWidth={1.5} />}
          disabled={test.isPending}
          onClick={() => test.mutate()}
        >
          {test.isPending ? "Sending…" : "Send test"}
        </Button>
        <IconButton label={`Edit ${channel.name}`} onClick={onEdit}>
          <Pencil size={16} strokeWidth={1.5} />
        </IconButton>
        <IconButton label={`Delete ${channel.name}`} onClick={onDelete}>
          <Trash size={16} strokeWidth={1.5} />
        </IconButton>
      </td>
    </tr>
  );
}

// ---- forms ----

function SeverityField({ value, onChange }: { value: Severity; onChange: (s: Severity) => void }) {
  return (
    <SelectField
      label="Minimum severity"
      hint="Alerts below this severity are not sent here."
      value={value}
      onChange={(e) => onChange(e.target.value as Severity)}
    >
      {SEVERITIES.map((s) => (
        <option key={s} value={s}>
          {SEVERITY_LABELS[s]}
        </option>
      ))}
    </SelectField>
  );
}

function ConfigFields({
  type,
  draft,
  errors,
  onChange,
}: {
  type: ChannelType;
  draft: ConfigDraft;
  errors: FieldErrors;
  onChange: (patch: Partial<ConfigDraft>) => void;
}) {
  const notify = useToast();

  switch (type) {
    case "email":
      return (
        <TextAreaField
          label="Recipients"
          hint={`One address per line or comma separated, up to ${MAX_RECIPIENTS}.`}
          rows={3}
          required
          value={draft.to}
          error={errors.to}
          onChange={(e) => onChange({ to: e.target.value })}
        />
      );
    case "slack":
    case "discord":
      return (
        <TextField
          label="Webhook URL"
          type="url"
          required
          autoComplete="off"
          placeholder={
            type === "slack"
              ? "https://hooks.slack.com/services/…"
              : "https://discord.com/api/webhooks/…"
          }
          hint={
            type === "slack"
              ? "An incoming webhook URL from your Slack app."
              : "From the Discord channel settings, under Integrations, Webhooks."
          }
          value={draft.url}
          error={errors.url}
          onChange={(e) => onChange({ url: e.target.value })}
        />
      );
    case "webhook":
      return (
        <>
          <TextField
            label="URL"
            type="url"
            required
            autoComplete="off"
            placeholder="https://example.com/hooks/hetzner-monitor"
            hint="Receives a JSON POST for every alert event."
            value={draft.url}
            error={errors.url}
            onChange={(e) => onChange({ url: e.target.value })}
          />
          <div className="stack" style={{ gap: 8 }}>
            <TextField
              label="Signing secret"
              required
              minLength={16}
              maxLength={256}
              autoComplete="off"
              spellCheck={false}
              hint="16 to 256 characters. The receiver checks the X-HM-Signature header (HMAC-SHA256 of the body) with it. Copy it now: it is not shown again."
              value={draft.secret}
              error={errors.secret}
              onChange={(e) => onChange({ secret: e.target.value })}
            />
            <div className="row">
              <Button
                size="sm"
                variant="outline"
                icon={<RefreshCw size={16} strokeWidth={1.5} />}
                onClick={() => onChange({ secret: generateSecret() })}
              >
                Generate
              </Button>
              {draft.secret && typeof navigator !== "undefined" && navigator.clipboard && (
                <Button
                  size="sm"
                  variant="ghost"
                  icon={<Copy size={16} strokeWidth={1.5} />}
                  onClick={() => {
                    navigator.clipboard.writeText(draft.secret).then(
                      () => notify("success", "Secret copied"),
                      () => notify("danger", "Could not copy", "Select the secret and copy it."),
                    );
                  }}
                >
                  Copy
                </Button>
              )}
            </div>
          </div>
        </>
      );
    case "web_push":
      return (
        <p className="small muted">
          Sends a push notification to every device a member of the org has turned push on for. Each
          person does that on their phone or computer under <Link to="/settings">Settings</Link>.
        </p>
      );
    case "ntfy":
      return (
        <>
          <TextField
            label="Server"
            type="url"
            autoComplete="off"
            placeholder="https://ntfy.sh"
            hint="Leave empty for ntfy.sh."
            value={draft.server}
            error={errors.server}
            onChange={(e) => onChange({ server: e.target.value })}
          />
          <TextField
            label="Topic"
            required
            maxLength={64}
            autoComplete="off"
            spellCheck={false}
            hint="Letters, digits, _ and -, up to 64 characters. On a public server, pick a name nobody can guess."
            value={draft.topic}
            error={errors.topic}
            onChange={(e) => onChange({ topic: e.target.value })}
          />
          <TextField
            label="Access token"
            type="password"
            autoComplete="off"
            hint="Optional, only for protected topics."
            value={draft.token}
            error={errors.token}
            onChange={(e) => onChange({ token: e.target.value })}
          />
        </>
      );
  }
}

function CreateChannelForm({ onDone }: { onDone: () => void }) {
  const queryClient = useQueryClient();
  const notify = useToast();
  const [type, setType] = useState<ChannelType>("email");
  const [name, setName] = useState("");
  const [minSeverity, setMinSeverity] = useState<Severity>("warning");
  const [enabled, setEnabled] = useState(true);
  const [draft, setDraft] = useState<ConfigDraft>(EMPTY_CONFIG);
  const [errors, setErrors] = useState<FieldErrors>({});
  const [formError, setFormError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Not a useMutation: its variables (with secrets) would stay in the mutation cache.
  async function submit(e: FormEvent) {
    e.preventDefault();
    setFormError(null);
    const trimmed = name.trim();
    const built = buildConfig(type, draft);
    const next: FieldErrors = { ...built.errors };
    if (!trimmed) next.name = "Enter a name.";
    if (!built.config || next.name) {
      setErrors(next);
      return;
    }
    setErrors({});
    setBusy(true);
    try {
      const { channel } = await unwrap(
        api.POST("/api/v1/channels", {
          body: { type, name: trimmed, config: built.config, min_severity: minSeverity, enabled },
        }),
      );
      setDraft(EMPTY_CONFIG);
      await queryClient.invalidateQueries({ queryKey: ["channels"] });
      notify("success", "Channel added", channel.name);
      onDone();
    } catch (err) {
      const mapped = saveError(err);
      if (mapped.name) setErrors({ name: mapped.name });
      setFormError(mapped.form ?? null);
      setBusy(false);
    }
  }

  return (
    <form className="form" onSubmit={submit} noValidate>
      <div className="form-row">
        <SelectField
          label="Type"
          value={type}
          onChange={(e) => {
            setType(e.target.value as ChannelType);
            setErrors({});
            setFormError(null);
          }}
        >
          {TYPES.map((t) => (
            <option key={t.type} value={t.type}>
              {t.label}
            </option>
          ))}
        </SelectField>
        <TextField
          label="Name"
          required
          maxLength={100}
          placeholder="Ops on-call"
          value={name}
          error={errors.name}
          onChange={(e) => setName(e.target.value)}
        />
      </div>
      <ConfigFields
        type={type}
        draft={draft}
        errors={errors}
        onChange={(patch) => setDraft((d) => ({ ...d, ...patch }))}
      />
      <SeverityField value={minSeverity} onChange={setMinSeverity} />
      <Switch checked={enabled} onChange={setEnabled} label="Enabled" />
      {formError && <ErrorNote>{formError}</ErrorNote>}
      <div className="dialog-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={busy}>
          {busy ? "Adding…" : "Add channel"}
        </Button>
      </div>
    </form>
  );
}

function EditChannelForm({ channel, onDone }: { channel: Channel; onDone: () => void }) {
  const queryClient = useQueryClient();
  const notify = useToast();
  const [name, setName] = useState(channel.name);
  const [minSeverity, setMinSeverity] = useState<Severity>(channel.min_severity);
  const [enabled, setEnabled] = useState(channel.enabled);
  const [replace, setReplace] = useState(false);
  const [draft, setDraft] = useState<ConfigDraft>(EMPTY_CONFIG);
  const [errors, setErrors] = useState<FieldErrors>({});
  const [formError, setFormError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setFormError(null);
    const trimmed = name.trim();
    const next: FieldErrors = {};
    if (!trimmed) next.name = "Enter a name.";
    const body: Schemas["UpdateChannel"] = { name: trimmed, min_severity: minSeverity, enabled };
    if (replace) {
      const built = buildConfig(channel.type, draft);
      if (built.config) body.config = built.config;
      else Object.assign(next, built.errors);
    }
    if (Object.keys(next).length > 0) {
      setErrors(next);
      return;
    }
    setErrors({});
    setBusy(true);
    try {
      const { channel: updated } = await unwrap(
        api.PATCH("/api/v1/channels/{id}", { params: { path: { id: channel.id } }, body }),
      );
      setDraft(EMPTY_CONFIG);
      await queryClient.invalidateQueries({ queryKey: ["channels"] });
      notify("success", "Channel saved", updated.name);
      onDone();
    } catch (err) {
      const mapped = saveError(err);
      if (mapped.name) setErrors({ name: mapped.name });
      setFormError(mapped.form ?? null);
      setBusy(false);
    }
  }

  return (
    <form className="form" onSubmit={submit} noValidate>
      <dl className="kv">
        <dt>Type</dt>
        <dd>
          <TypeLabel type={channel.type} />
        </dd>
        <dt>Current target</dt>
        <dd className="mono">{channel.hint}</dd>
      </dl>
      <TextField
        label="Name"
        required
        maxLength={100}
        value={name}
        error={errors.name}
        onChange={(e) => setName(e.target.value)}
      />
      <SeverityField value={minSeverity} onChange={setMinSeverity} />
      <Switch checked={enabled} onChange={setEnabled} label="Enabled" />
      {channel.type !== "web_push" && (
        <Switch
          checked={replace}
          onChange={(v) => {
            setReplace(v);
            // Turning it off drops whatever was typed, secrets included.
            if (!v) {
              setDraft(EMPTY_CONFIG);
              setErrors({});
            }
          }}
          label="Replace configuration"
        />
      )}
      {replace && (
        <>
          <p className="small muted">
            The stored configuration is not shown. What you enter here replaces it completely.
          </p>
          <ConfigFields
            type={channel.type}
            draft={draft}
            errors={errors}
            onChange={(patch) => setDraft((d) => ({ ...d, ...patch }))}
          />
        </>
      )}
      {formError && <ErrorNote>{formError}</ErrorNote>}
      <div className="dialog-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={busy}>
          {busy ? "Saving…" : "Save"}
        </Button>
      </div>
    </form>
  );
}

function DeleteChannel({ channel, onDone }: { channel: Channel; onDone: () => void }) {
  const queryClient = useQueryClient();
  const notify = useToast();
  const remove = useMutation({
    mutationFn: () =>
      unwrap(api.DELETE("/api/v1/channels/{id}", { params: { path: { id: channel.id } } })),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["channels"] }),
        queryClient.invalidateQueries({ queryKey: ["rules"] }),
        queryClient.invalidateQueries({ queryKey: ["rule"] }),
      ]);
      notify("success", "Channel deleted", channel.name);
      onDone();
    },
  });

  return (
    <div className="form">
      <p>
        Delete <strong>{channel.name}</strong>? Alerts are no longer sent to it, and rules pinned
        only to this channel go back to routing by severity. This cannot be undone.
      </p>
      {remove.isError && <ErrorNote>{errorText(remove.error)}</ErrorNote>}
      <div className="dialog-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button
          variant="danger"
          icon={<Trash size={16} strokeWidth={1.5} />}
          disabled={remove.isPending}
          onClick={() => remove.mutate()}
        >
          {remove.isPending ? "Deleting…" : "Delete channel"}
        </Button>
      </div>
    </div>
  );
}
