// Rules: list, enable/disable, edit (incl. pinned channels), create and delete custom rules.
import { useState } from "react";
import type { FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useSearchParams } from "react-router";
import { Pencil, Plus, SlidersHorizontal, Trash2 } from "lucide-react";
import { ApiError, api, errorText, unwrap } from "../api/client";
import type { Schemas } from "../api/client";
import { CHECK_LABELS, SEVERITIES, severityTone } from "../format";
import type { CheckType, Severity } from "../format";
import {
  Badge,
  Button,
  Dialog,
  Empty,
  ErrorNote,
  PageHead,
  SelectField,
  Spinner,
  Switch,
  TextAreaField,
  TextField,
} from "../ui/components";
import { useToast } from "../ui/toast";

type Rule = Schemas["RuleView"];
type RuleList = Schemas["RuleList"];
type Project = Schemas["ProjectView"];
type Params = NonNullable<Schemas["CreateRule"]["params"]>;

/** Parameter templates and default severities per check type (docs/design/checks.md). */
const TEMPLATES: Record<
  Exclude<CheckType, "hetzner_api">,
  { params: object; severity: Severity }
> = {
  server_status: { params: { for_s: 120, transitional_grace_s: 600 }, severity: "critical" },
  resource_lifecycle: { params: {}, severity: "info" },
  lb_target_health: { params: { for_s: 120 }, severity: "critical" },
  traffic_quota: { params: { threshold_pct: 80, for_s: 0 }, severity: "warning" },
  cpu: { params: { threshold_pct: 90, for_s: 600 }, severity: "warning" },
  network: {
    params: { metric: "bandwidth_out", threshold: 10000000, for_s: 300 },
    severity: "warning",
  },
  disk_io: { params: { metric: "iops_read", threshold: 1000, for_s: 300 }, severity: "warning" },
  certificate_expiry: { params: { days: 14, on_failure: true }, severity: "warning" },
  deprecation: { params: {}, severity: "info" },
  hygiene_backups: { params: { for_s: 3600 }, severity: "info" },
  hygiene_protection: { params: { kinds: ["server"], for_s: 3600 }, severity: "info" },
  hygiene_orphans: { params: { for_s: 86400 }, severity: "info" },
  cost_change: { params: { threshold_pct: 10 }, severity: "info" },
};

const CREATABLE = Object.keys(TEMPLATES) as Exclude<CheckType, "hetzner_api">[];

const SELECTOR_HINT = "Hetzner label selector, e.g. env=prod. Leave empty for all resources.";

function pretty(value: unknown): string {
  return JSON.stringify(value, null, 2);
}

/** Parses the params textarea; returns an error text instead when it is not a JSON object. */
function parseParams(text: string): { params: Params } | { error: string } {
  const trimmed = text.trim();
  if (!trimmed) return { params: {} as Params };
  let value: unknown;
  try {
    value = JSON.parse(trimmed);
  } catch {
    return { error: 'Params are not valid JSON. Example: {"for_s": 300}' };
  }
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return { error: 'Params must be a JSON object. Example: {"for_s": 300}' };
  }
  return { params: value as Params };
}

export function RulesPage() {
  const [search, setSearch] = useSearchParams();
  const projectId = search.get("project_id") ?? "";
  const [creating, setCreating] = useState(false);
  const [editing, setEditing] = useState<Rule | null>(null);
  const [deleting, setDeleting] = useState<Rule | null>(null);
  const notify = useToast();
  const qc = useQueryClient();

  const rules = useQuery({
    queryKey: ["rules", projectId || "all"],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/rules", {
          params: { query: projectId ? { project_id: projectId } : {} },
        }),
      ),
  });
  const projects = useQuery({
    queryKey: ["projects"],
    queryFn: () => unwrap(api.GET("/api/v1/projects")),
  });
  const projectList = projects.data?.projects ?? [];
  const projectsNote = projects.isError
    ? "Projects could not be loaded, so only All projects is offered."
    : undefined;
  // While the names load, or if they failed, show the id prefix instead of a misleading
  // "Unknown project": the rule does belong to a project.
  const projectName = (id: string | null) => {
    if (!id) return "All projects";
    const found = projectList.find((p) => p.id === id);
    if (found) return found.name;
    if (projects.isPending) return "…";
    if (projects.isError) return `Project ${id.slice(0, 8)}`;
    return "Unknown project";
  };

  const toggle = useMutation({
    mutationFn: ({ id, enabled }: { id: string; enabled: boolean }) =>
      unwrap(api.PATCH("/api/v1/rules/{id}", { params: { path: { id } }, body: { enabled } })),
    onMutate: async ({ id, enabled }) => {
      await qc.cancelQueries({ queryKey: ["rules"] });
      const before = qc.getQueriesData<RuleList>({ queryKey: ["rules"] });
      qc.setQueriesData<RuleList>({ queryKey: ["rules"] }, (old) =>
        old ? { rules: old.rules.map((r) => (r.id === id ? { ...r, enabled } : r)) } : old,
      );
      return { before };
    },
    onError: (e, _vars, ctx) => {
      ctx?.before.forEach(([key, data]) => qc.setQueryData(key, data));
      notify("danger", "Could not change the rule", errorText(e));
    },
    onSettled: (_data, _e, { id }) => {
      void qc.invalidateQueries({ queryKey: ["rules"] });
      void qc.invalidateQueries({ queryKey: ["rule", id] });
    },
  });

  function setProject(id: string) {
    const next = new URLSearchParams(search);
    if (id) next.set("project_id", id);
    else next.delete("project_id");
    setSearch(next, { replace: true });
  }

  const list = rules.data?.rules ?? [];

  return (
    <>
      <PageHead
        title="Rules"
        description="What is checked, how severe it is and where it is sent. Default rules can be disabled, not deleted."
        actions={
          <Button
            variant="primary"
            icon={<Plus size={16} strokeWidth={1.5} aria-hidden="true" />}
            onClick={() => setCreating(true)}
          >
            New rule
          </Button>
        }
      />

      <div className="row" style={{ marginBottom: 16 }}>
        <div style={{ minWidth: 220 }}>
          <SelectField
            label="Project"
            value={projectId}
            onChange={(e) => setProject(e.target.value)}
          >
            <option value="">All projects</option>
            {projectList.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </SelectField>
        </div>
      </div>

      {projects.isError && (
        <ErrorNote>Could not load the project names: {errorText(projects.error)}</ErrorNote>
      )}

      {rules.isPending ? (
        <Spinner label="Loading rules" />
      ) : rules.isError ? (
        <ErrorNote>{errorText(rules.error)}</ErrorNote>
      ) : list.length === 0 ? (
        <Empty title="No rules" icon={<SlidersHorizontal size={24} strokeWidth={1.5} />}>
          <p className="muted">
            Default rules are created when a project is connected. Add your own with New rule.
          </p>
        </Empty>
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>On</th>
                <th>Name</th>
                <th>Check</th>
                <th>Severity</th>
                <th className="hide-mobile">Scope</th>
                <th className="actions">
                  <span className="visually-hidden">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {list.map((rule) => (
                <tr key={rule.id}>
                  <td>
                    <Switch
                      checked={rule.enabled}
                      onChange={(enabled) => toggle.mutate({ id: rule.id, enabled })}
                      label={<span className="visually-hidden">Enable {rule.name}</span>}
                    />
                  </td>
                  <td className="strong">
                    <div className="row" style={{ gap: 8 }}>
                      <span>{rule.name}</span>
                      {rule.is_default && <Badge>default</Badge>}
                    </div>
                  </td>
                  <td>{CHECK_LABELS[rule.check_type]}</td>
                  <td>
                    <Badge tone={severityTone(rule.severity)} dot>
                      {rule.severity}
                    </Badge>
                  </td>
                  <td className="hide-mobile">
                    <div className="stack" style={{ gap: 2 }}>
                      <span>{projectName(rule.project_id)}</span>
                      {rule.selector && <span className="mono small">{rule.selector}</span>}
                      {rule.resource_id && (
                        <span className="mono small muted" title={rule.resource_id}>
                          resource {rule.resource_id.slice(0, 8)}
                        </span>
                      )}
                    </div>
                  </td>
                  <td className="actions">
                    <Button
                      size="sm"
                      variant="ghost"
                      icon={<Pencil size={16} strokeWidth={1.5} aria-hidden="true" />}
                      onClick={() => setEditing(rule)}
                      aria-label={`Edit ${rule.name}`}
                    >
                      Edit
                    </Button>
                    {!rule.is_default && (
                      <Button
                        size="sm"
                        variant="ghost"
                        icon={<Trash2 size={16} strokeWidth={1.5} aria-hidden="true" />}
                        onClick={() => setDeleting(rule)}
                        aria-label={`Delete ${rule.name}`}
                      >
                        Delete
                      </Button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}

      <Dialog open={creating} onClose={() => setCreating(false)} title="New rule">
        <CreateRuleForm
          projects={projectList}
          projectsNote={projectsNote}
          initialProject={projectId}
          onDone={() => setCreating(false)}
        />
      </Dialog>

      <Dialog open={editing !== null} onClose={() => setEditing(null)} title="Edit rule">
        {editing && <EditRule rule={editing} onDone={() => setEditing(null)} />}
      </Dialog>

      <Dialog open={deleting !== null} onClose={() => setDeleting(null)} title="Delete rule?">
        {deleting && <DeleteRule rule={deleting} onDone={() => setDeleting(null)} />}
      </Dialog>
    </>
  );
}

function CreateRuleForm({
  projects,
  projectsNote,
  initialProject,
  onDone,
}: {
  projects: Project[];
  projectsNote?: string;
  initialProject: string;
  onDone: () => void;
}) {
  const qc = useQueryClient();
  const notify = useToast();
  const [projectId, setProjectId] = useState(initialProject);
  const [name, setName] = useState("");
  const [checkType, setCheckType] = useState<Exclude<CheckType, "hetzner_api">>("cpu");
  const [severity, setSeverity] = useState<Severity>(TEMPLATES.cpu.severity);
  const [selector, setSelector] = useState("");
  const [paramsText, setParamsText] = useState(pretty(TEMPLATES.cpu.params));
  const [enabled, setEnabled] = useState(true);
  const [paramsError, setParamsError] = useState<string | null>(null);

  const create = useMutation({
    mutationFn: (body: Schemas["CreateRule"]) => unwrap(api.POST("/api/v1/rules", { body })),
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["rules"] });
      void qc.invalidateQueries({ queryKey: ["projects"] });
      notify("success", "Rule created");
      onDone();
    },
  });

  function changeCheck(next: Exclude<CheckType, "hetzner_api">) {
    setCheckType(next);
    setSeverity(TEMPLATES[next].severity);
    setParamsText(pretty(TEMPLATES[next].params));
    setParamsError(null);
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    const parsed = parseParams(paramsText);
    if ("error" in parsed) {
      setParamsError(parsed.error);
      return;
    }
    setParamsError(null);
    const body: Schemas["CreateRule"] = {
      name: name.trim(),
      check_type: checkType,
      severity,
      params: parsed.params,
      enabled,
    };
    if (projectId) body.project_id = projectId;
    if (selector.trim()) body.selector = selector.trim();
    create.mutate(body);
  }

  return (
    <form className="form" onSubmit={submit} noValidate>
      <SelectField
        label="Project"
        value={projectId}
        hint={projectsNote}
        onChange={(e) => setProjectId(e.target.value)}
      >
        <option value="">All projects</option>
        {projects.map((p) => (
          <option key={p.id} value={p.id}>
            {p.name}
          </option>
        ))}
      </SelectField>
      <TextField
        label="Name"
        required
        maxLength={200}
        value={name}
        onChange={(e) => setName(e.target.value)}
        placeholder="CPU high on web servers"
      />
      <div className="form-row">
        <SelectField
          label="Check"
          value={checkType}
          onChange={(e) => changeCheck(e.target.value as Exclude<CheckType, "hetzner_api">)}
        >
          {CREATABLE.map((c) => (
            <option key={c} value={c}>
              {CHECK_LABELS[c]}
            </option>
          ))}
        </SelectField>
        <SelectField
          label="Severity"
          value={severity}
          onChange={(e) => setSeverity(e.target.value as Severity)}
        >
          {SEVERITIES.map((s) => (
            <option key={s} value={s}>
              {s}
            </option>
          ))}
        </SelectField>
      </div>
      <TextField
        label="Selector"
        className="input mono"
        value={selector}
        onChange={(e) => setSelector(e.target.value)}
        hint={SELECTOR_HINT}
        autoComplete="off"
        spellCheck={false}
      />
      <TextAreaField
        label="Params"
        mono
        rows={6}
        value={paramsText}
        onChange={(e) => setParamsText(e.target.value)}
        error={paramsError}
        hint="JSON. Prefilled with the template of the chosen check."
        spellCheck={false}
      />
      <Switch checked={enabled} onChange={setEnabled} label="Enabled" />
      {create.isError && <ErrorNote>{errorText(create.error)}</ErrorNote>}
      <div className="dialog-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={create.isPending || name.trim() === ""}>
          {create.isPending ? "Creating…" : "Create rule"}
        </Button>
      </div>
    </form>
  );
}

function EditRule({ rule, onDone }: { rule: Rule; onDone: () => void }) {
  const detail = useQuery({
    queryKey: ["rule", rule.id],
    queryFn: () => unwrap(api.GET("/api/v1/rules/{id}", { params: { path: { id: rule.id } } })),
  });
  const channels = useQuery({
    queryKey: ["channels"],
    queryFn: () => unwrap(api.GET("/api/v1/channels")),
  });

  if (detail.isPending || channels.isPending) return <Spinner label="Loading rule" />;
  if (detail.isError) return <ErrorNote>{errorText(detail.error)}</ErrorNote>;
  if (channels.isError) return <ErrorNote>{errorText(channels.error)}</ErrorNote>;
  return (
    <EditRuleForm
      rule={detail.data.rule}
      channelIds={detail.data.channel_ids}
      channels={channels.data.channels}
      onDone={onDone}
    />
  );
}

function EditRuleForm({
  rule,
  channelIds,
  channels,
  onDone,
}: {
  rule: Rule;
  channelIds: string[];
  channels: Schemas["ChannelView"][];
  onDone: () => void;
}) {
  const qc = useQueryClient();
  const notify = useToast();
  const [name, setName] = useState(rule.name);
  const [severity, setSeverity] = useState<Severity>(rule.severity);
  const [selector, setSelector] = useState(rule.selector ?? "");
  const [paramsText, setParamsText] = useState(pretty(rule.params ?? {}));
  const [pinned, setPinned] = useState<string[]>(channelIds);
  const [paramsError, setParamsError] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: (body: Schemas["UpdateRule"]) =>
      unwrap(api.PATCH("/api/v1/rules/{id}", { params: { path: { id: rule.id } }, body })),
    onSuccess: (data) => {
      qc.setQueryData(["rule", rule.id], data);
      void qc.invalidateQueries({ queryKey: ["rules"] });
      void qc.invalidateQueries({ queryKey: ["projects"] });
      notify("success", "Rule saved");
      onDone();
    },
  });

  function togglePin(id: string, on: boolean) {
    setPinned((all) => (on ? [...all, id] : all.filter((c) => c !== id)));
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    const parsed = parseParams(paramsText);
    if ("error" in parsed) {
      setParamsError(parsed.error);
      return;
    }
    setParamsError(null);
    save.mutate({
      name: name.trim(),
      severity,
      selector: selector.trim() || null,
      params: parsed.params,
      channel_ids: pinned,
    });
  }

  return (
    <form className="form" onSubmit={submit} noValidate>
      <p className="small muted">
        {CHECK_LABELS[rule.check_type]}
        {rule.is_default && " · default rule"}
      </p>
      <TextField
        label="Name"
        required
        maxLength={200}
        value={name}
        onChange={(e) => setName(e.target.value)}
      />
      <SelectField
        label="Severity"
        value={severity}
        onChange={(e) => setSeverity(e.target.value as Severity)}
      >
        {SEVERITIES.map((s) => (
          <option key={s} value={s}>
            {s}
          </option>
        ))}
      </SelectField>
      <TextField
        label="Selector"
        className="input mono"
        value={selector}
        onChange={(e) => setSelector(e.target.value)}
        hint="Hetzner label selector, e.g. env=prod. Leave empty to clear it."
        autoComplete="off"
        spellCheck={false}
      />
      <TextAreaField
        label="Params"
        mono
        rows={6}
        value={paramsText}
        onChange={(e) => setParamsText(e.target.value)}
        error={paramsError}
        hint="JSON. The server checks the fields for this check type."
        spellCheck={false}
      />
      <fieldset className="field" style={{ border: 0, padding: 0, margin: 0 }}>
        <legend className="label">Pinned channels</legend>
        {channels.length === 0 ? (
          <span className="hint">No channels yet. Add one under Channels.</span>
        ) : (
          <div className="checks">
            {channels.map((c) => (
              <label key={c.id} className="check">
                <input
                  type="checkbox"
                  checked={pinned.includes(c.id)}
                  onChange={(e) => togglePin(c.id, e.target.checked)}
                />
                {c.name}
              </label>
            ))}
          </div>
        )}
        <span className="hint">
          No channel pinned = each channel gets the alert by its minimum severity.
        </span>
      </fieldset>
      {save.isError && <ErrorNote>{errorText(save.error)}</ErrorNote>}
      <div className="dialog-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={save.isPending || name.trim() === ""}>
          {save.isPending ? "Saving…" : "Save rule"}
        </Button>
      </div>
    </form>
  );
}

function DeleteRule({ rule, onDone }: { rule: Rule; onDone: () => void }) {
  const qc = useQueryClient();
  const notify = useToast();
  const remove = useMutation({
    mutationFn: () =>
      unwrap(api.DELETE("/api/v1/rules/{id}", { params: { path: { id: rule.id } } })),
    onSuccess: () => {
      qc.removeQueries({ queryKey: ["rule", rule.id] });
      void qc.invalidateQueries({ queryKey: ["rules"] });
      void qc.invalidateQueries({ queryKey: ["alerts"] });
      notify("success", "Rule deleted");
      onDone();
    },
    onError: (e) => {
      if (e instanceof ApiError && e.code === "default_rule") {
        notify("warning", "Default rules can be disabled, not deleted");
        onDone();
      }
    },
  });

  const otherError =
    remove.isError && !(remove.error instanceof ApiError && remove.error.code === "default_rule");

  return (
    <div className="form">
      <p>
        <strong>{rule.name}</strong> and all of its alerts are deleted. This cannot be undone.
      </p>
      {otherError && <ErrorNote>{errorText(remove.error)}</ErrorNote>}
      <div className="dialog-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button variant="danger" disabled={remove.isPending} onClick={() => remove.mutate()}>
          {remove.isPending ? "Deleting…" : "Delete rule"}
        </Button>
      </div>
    </div>
  );
}
