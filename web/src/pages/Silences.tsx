// Silences: list (active or all), create, end now.
import { useState } from "react";
import type { FormEvent, ReactNode } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { BellOff, Plus } from "lucide-react";
import { api, errorText, unwrap, type Schemas } from "../api/client";
import { dateTime, relTime } from "../format";
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
import type { Tone } from "../ui/components";
import { useToast } from "../ui/toast";

type Silence = Schemas["SilenceView"];
type Status = "active" | "scheduled" | "ended";

const HOUR = 3600_000;
const DAY = 24 * HOUR;
const MAX_SPAN = 365 * DAY;

const PRESETS: { value: string; label: string; ms: number | null }[] = [
  { value: "1h", label: "1 hour", ms: HOUR },
  { value: "4h", label: "4 hours", ms: 4 * HOUR },
  { value: "1d", label: "1 day", ms: DAY },
  { value: "1w", label: "1 week", ms: 7 * DAY },
  { value: "custom", label: "Custom end", ms: null },
];

const STATUS_TONE: Record<Status, Tone> = { active: "aqua", scheduled: "info", ended: "neutral" };

function silenceStatus(s: Silence, now: Date = new Date()): Status {
  const t = now.getTime();
  if (new Date(s.ends_at).getTime() <= t) return "ended";
  if (new Date(s.starts_at).getTime() > t) return "scheduled";
  return "active";
}

function When({ iso }: { iso: string }) {
  return <span title={dateTime(iso)}>{relTime(iso)}</span>;
}

export function SilencesPage() {
  const [showEnded, setShowEnded] = useState(false);
  const [creating, setCreating] = useState(false);
  const [ending, setEnding] = useState<Silence | null>(null);

  const silences = useQuery({
    queryKey: ["silences", { active: !showEnded }],
    queryFn: () =>
      unwrap(api.GET("/api/v1/silences", { params: { query: { active: !showEnded } } })),
  });
  const projects = useQuery({
    queryKey: ["projects"],
    queryFn: () => unwrap(api.GET("/api/v1/projects")),
  });
  const rules = useQuery({
    queryKey: ["rules", "all"],
    queryFn: () => unwrap(api.GET("/api/v1/rules")),
  });

  const projectName = (id: string) =>
    projects.data?.projects.find((p) => p.id === id)?.name ?? "Unknown project";
  const ruleName = (id: string) =>
    rules.data?.rules.find((r) => r.id === id)?.name ?? "Unknown rule";

  function scope(s: Silence): ReactNode {
    const parts: ReactNode[] = [];
    if (s.project_id) parts.push(<span key="p">Project {projectName(s.project_id)}</span>);
    if (s.rule_id) parts.push(<span key="r">Rule {ruleName(s.rule_id)}</span>);
    if (s.resource_id)
      parts.push(
        <span key="res">
          Resource <span className="mono">{s.resource_id.slice(0, 8)}</span>
        </span>,
      );
    if (s.selector)
      parts.push(
        <span key="sel" className="mono">
          {s.selector}
        </span>,
      );
    if (parts.length === 0) return <span>Whole org</span>;
    return (
      <div className="stack" style={{ gap: 2 }}>
        {parts}
      </div>
    );
  }

  const list = silences.data?.silences ?? [];

  return (
    <>
      <PageHead
        title="Silences"
        description="A silence mutes notifications for its scope until it ends. Alerts still open and resolve."
        actions={
          <Button
            variant="primary"
            icon={<Plus size={16} strokeWidth={1.5} />}
            onClick={() => setCreating(true)}
          >
            New silence
          </Button>
        }
      />
      <div className="row" style={{ marginBottom: 16 }}>
        <Switch checked={showEnded} onChange={setShowEnded} label="Show ended" />
      </div>

      {silences.isPending ? (
        <Spinner label="Loading silences" />
      ) : silences.isError ? (
        <ErrorNote>Could not load silences: {errorText(silences.error)}</ErrorNote>
      ) : list.length === 0 ? (
        <Empty
          title={showEnded ? "No silences" : "No active silences"}
          icon={<BellOff size={24} strokeWidth={1.5} />}
        >
          <p className="muted small">Create one before planned maintenance to keep things quiet.</p>
        </Empty>
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Scope</th>
                <th className="hide-mobile">Reason</th>
                <th className="hide-mobile">Starts</th>
                <th>Ends</th>
                <th className="hide-mobile">Created</th>
                <th>Status</th>
                <th className="actions">
                  <span className="visually-hidden">Actions</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {list.map((s) => {
                const status = silenceStatus(s);
                return (
                  <tr key={s.id}>
                    <td className="strong">{scope(s)}</td>
                    <td className="hide-mobile">{s.reason || <span className="subtle">—</span>}</td>
                    <td className="hide-mobile">
                      <When iso={s.starts_at} />
                    </td>
                    <td>
                      <When iso={s.ends_at} />
                    </td>
                    <td className="hide-mobile">
                      <When iso={s.created_at} />
                    </td>
                    <td>
                      <Badge tone={STATUS_TONE[status]} dot={status === "active"}>
                        {status === "active"
                          ? "Active"
                          : status === "scheduled"
                            ? "Scheduled"
                            : "Ended"}
                      </Badge>
                    </td>
                    <td className="actions">
                      {status !== "ended" && (
                        <Button size="sm" variant="outline" onClick={() => setEnding(s)}>
                          {status === "scheduled" ? "Remove" : "End now"}
                        </Button>
                      )}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      )}

      <Dialog open={creating} onClose={() => setCreating(false)} title="New silence">
        <NewSilenceForm
          projects={projects.data?.projects ?? []}
          onDone={() => setCreating(false)}
        />
      </Dialog>

      <Dialog
        open={ending !== null}
        onClose={() => setEnding(null)}
        title={
          ending && silenceStatus(ending) === "scheduled" ? "Remove silence?" : "End silence now?"
        }
      >
        {ending && <EndSilence silence={ending} onDone={() => setEnding(null)} />}
      </Dialog>
    </>
  );
}

function EndSilence({ silence, onDone }: { silence: Silence; onDone: () => void }) {
  const queryClient = useQueryClient();
  const notify = useToast();
  const scheduled = silenceStatus(silence) === "scheduled";
  const end = useMutation({
    mutationFn: () =>
      unwrap(api.DELETE("/api/v1/silences/{id}", { params: { path: { id: silence.id } } })),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["silences"] });
      notify("success", scheduled ? "Silence removed" : "Silence ended");
      onDone();
    },
  });
  return (
    <div className="stack">
      <p className="muted">
        {scheduled
          ? "The silence has not started yet and will be removed."
          : "Notifications for this scope start again right away."}
      </p>
      {end.isError && <ErrorNote>{errorText(end.error)}</ErrorNote>}
      <div className="dialog-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button variant="danger" disabled={end.isPending} onClick={() => end.mutate()}>
          {scheduled ? "Remove silence" : "End now"}
        </Button>
      </div>
    </div>
  );
}

function NewSilenceForm({
  projects,
  onDone,
}: {
  projects: Schemas["ProjectView"][];
  onDone: () => void;
}) {
  const queryClient = useQueryClient();
  const notify = useToast();
  const [projectId, setProjectId] = useState("");
  const [ruleId, setRuleId] = useState("");
  const [selector, setSelector] = useState("");
  const [starts, setStarts] = useState("");
  const [preset, setPreset] = useState("1h");
  const [ends, setEnds] = useState("");
  const [reason, setReason] = useState("");
  const [fieldError, setFieldError] = useState<{ starts?: string; ends?: string }>({});

  const rules = useQuery({
    // Same keys as the rules page, so both share the cache.
    queryKey: ["rules", projectId || "all"],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/rules", {
          params: { query: projectId ? { project_id: projectId } : {} },
        }),
      ),
  });
  const ruleOptions = rules.data?.rules ?? [];

  const create = useMutation({
    mutationFn: (body: Schemas["CreateSilence"]) => unwrap(api.POST("/api/v1/silences", { body })),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["silences"] });
      notify("success", "Silence created");
      onDone();
    },
  });

  function submit(e: FormEvent) {
    e.preventDefault();
    const now = Date.now();
    const start = starts ? new Date(starts) : new Date(now);
    if (Number.isNaN(start.getTime())) {
      setFieldError({ starts: "Enter a valid start time." });
      return;
    }
    const ms = PRESETS.find((p) => p.value === preset)?.ms ?? null;
    const end = ms === null ? new Date(ends) : new Date(start.getTime() + ms);
    let endError: string | undefined;
    if (Number.isNaN(end.getTime())) endError = "Enter a valid end time.";
    else if (end.getTime() <= now) endError = "The end must be in the future.";
    else if (end.getTime() <= start.getTime()) endError = "The end must be after the start.";
    else if (end.getTime() - start.getTime() > MAX_SPAN)
      endError = "A silence lasts at most 365 days.";
    if (endError) {
      setFieldError({ ends: endError });
      return;
    }
    setFieldError({});
    const body: Schemas["CreateSilence"] = { ends_at: end.toISOString() };
    if (starts) body.starts_at = start.toISOString();
    if (projectId) body.project_id = projectId;
    if (ruleId) body.rule_id = ruleId;
    if (selector.trim()) body.selector = selector.trim();
    if (reason.trim()) body.reason = reason.trim();
    create.mutate(body);
  }

  const custom = preset === "custom";

  return (
    <form className="form" onSubmit={submit} noValidate>
      <p className="muted small">Leave the scope empty to silence the whole org.</p>
      <div className="form-row">
        <SelectField
          label="Project"
          value={projectId}
          onChange={(e) => {
            setProjectId(e.target.value);
            setRuleId("");
          }}
        >
          <option value="">All projects</option>
          {projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </SelectField>
        <SelectField
          label="Rule"
          value={ruleId}
          onChange={(e) => setRuleId(e.target.value)}
          hint={rules.isError ? "Could not load rules." : undefined}
        >
          <option value="">All rules</option>
          {ruleOptions.map((r) => (
            <option key={r.id} value={r.id}>
              {r.name}
            </option>
          ))}
        </SelectField>
      </div>
      <TextField
        label="Selector"
        className="input mono"
        placeholder="env=prod,role!=db"
        hint="Optional. Matches resource labels, like rule selectors."
        value={selector}
        onChange={(e) => setSelector(e.target.value)}
      />
      <div className="form-row">
        <TextField
          label="Starts"
          type="datetime-local"
          hint="Leave empty to start now."
          error={fieldError.starts}
          value={starts}
          onChange={(e) => setStarts(e.target.value)}
        />
        <SelectField label="Duration" value={preset} onChange={(e) => setPreset(e.target.value)}>
          {PRESETS.map((p) => (
            <option key={p.value} value={p.value}>
              {p.label}
            </option>
          ))}
        </SelectField>
      </div>
      {custom ? (
        <TextField
          label="Ends"
          type="datetime-local"
          required
          hint="At most 365 days after the start."
          error={fieldError.ends}
          value={ends}
          onChange={(e) => setEnds(e.target.value)}
        />
      ) : (
        fieldError.ends && <ErrorNote>{fieldError.ends}</ErrorNote>
      )}
      <TextAreaField
        label="Reason"
        rows={2}
        maxLength={500}
        placeholder="Kernel update on the web servers"
        value={reason}
        onChange={(e) => setReason(e.target.value)}
      />
      {create.isError && <ErrorNote>{errorText(create.error)}</ErrorNote>}
      <div className="dialog-actions">
        <Button variant="ghost" onClick={onDone}>
          Cancel
        </Button>
        <Button type="submit" variant="primary" disabled={create.isPending}>
          {create.isPending ? "Creating…" : "Create silence"}
        </Button>
      </div>
    </form>
  );
}
