// Projects: list and connect (ProjectsPage), schedule, budget, cost and inventory (ProjectDetailPage).
import { useState } from "react";
import type { FormEvent } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import { Bell, FolderKanban, Plus, RefreshCw, Trash2 } from "lucide-react";
import { api, ApiError, errorText, unwrap, type Schemas } from "../api/client";
import {
  KIND_LABELS,
  dateTime,
  duration,
  money,
  projectStatusTone,
  relTime,
  type ResourceKind,
} from "../format";
import {
  Badge,
  Button,
  Card,
  Dialog,
  Empty,
  ErrorNote,
  PageHead,
  SelectField,
  Spinner,
  TextField,
} from "../ui/components";
import { useToast } from "../ui/toast";

type Project = Schemas["ProjectView"];
type ProjectStatus = Schemas["ProjectStatus"];
type SyncReport = Schemas["SyncReport"];

const STATUS_LABELS: Record<ProjectStatus, string> = {
  active: "Active",
  degraded: "Degraded",
  invalid_token: "Invalid token",
  paused: "Paused",
};

const KINDS = Object.keys(KIND_LABELS) as ResourceKind[];

function StatusBadge({ status }: { status: ProjectStatus }) {
  return (
    <Badge tone={projectStatusTone(status)} dot>
      {STATUS_LABELS[status]}
    </Badge>
  );
}

function tokenHint(hint: string): string {
  return `…${hint}`;
}

/** A relative time with the absolute time as tooltip; "—" when missing. */
function When({ at }: { at: string | null | undefined }) {
  if (!at) return <span className="subtle">—</span>;
  return (
    <time dateTime={at} title={dateTime(at)}>
      {relTime(at)}
    </time>
  );
}

/** "12 resources (3 new, 1 deleted)" plus a per-kind breakdown. */
function syncSummary(report: SyncReport): { title: string; detail?: string } {
  let seen = 0;
  let added = 0;
  let deleted = 0;
  const parts: string[] = [];
  for (const kind of KINDS) {
    const counts = report[kind];
    if (!counts) continue;
    seen += counts.seen;
    added += counts.new;
    deleted += counts.deleted;
    if (counts.seen > 0) parts.push(`${KIND_LABELS[kind]}: ${counts.seen}`);
  }
  const changes = [added > 0 && `${added} new`, deleted > 0 && `${deleted} deleted`].filter(
    Boolean,
  );
  const title = `${seen} ${seen === 1 ? "resource" : "resources"} found${
    changes.length ? ` (${changes.join(", ")})` : ""
  }`;
  return { title, detail: parts.length ? parts.join(" · ") : undefined };
}

// ---------------------------------------------------------------------------------------------
// List and connect

export function ProjectsPage() {
  const [connectOpen, setConnectOpen] = useState(false);
  const projects = useQuery({
    queryKey: ["projects"],
    // Cache the whole `ProjectList` like the other pages do; this page only needs the array.
    queryFn: () => unwrap(api.GET("/api/v1/projects")),
    select: (d) => d.projects,
  });

  return (
    <>
      <PageHead
        title="Projects"
        description="Hetzner Cloud projects this organisation monitors, each with its own read-only token."
        actions={
          <Button
            variant="primary"
            icon={<Plus size={16} strokeWidth={1.5} />}
            onClick={() => setConnectOpen(true)}
          >
            Connect project
          </Button>
        }
      />
      {projects.isPending ? (
        <Spinner label="Loading projects" />
      ) : projects.isError ? (
        <ErrorNote>Could not load the projects: {errorText(projects.error)}</ErrorNote>
      ) : projects.data.length === 0 ? (
        <Empty title="No project connected yet" icon={<FolderKanban size={24} strokeWidth={1.5} />}>
          <p className="muted">
            Create a Read API token in the Hetzner Console and connect the project. The first
            inventory sync runs right away and the default rules start watching it.
          </p>
        </Empty>
      ) : (
        <div className="grid">
          {projects.data.map((p) => (
            <ProjectCard key={p.id} project={p} />
          ))}
        </div>
      )}
      <ConnectDialog open={connectOpen} onClose={() => setConnectOpen(false)} />
    </>
  );
}

function ProjectCard({ project: p }: { project: Project }) {
  return (
    <Link to={`/projects/${p.id}`} className="card card-link">
      <div className="row">
        <h2 className="card-title">{p.name}</h2>
        <span className="spacer" />
        <StatusBadge status={p.status} />
      </div>
      {p.status_message && <p className="small muted">{p.status_message}</p>}
      <dl className="kv">
        <dt>Token</dt>
        <dd className="mono">{tokenHint(p.token_hint)}</dd>
        <dt>Last inventory</dt>
        <dd>
          <When at={p.last_inventory_at} />
        </dd>
        <dt>Budget</dt>
        <dd>
          {p.budget_h != null && p.rl_limit != null
            ? `${p.budget_h} of ${p.rl_limit} requests/h`
            : "After the first request"}
        </dd>
      </dl>
    </Link>
  );
}

type FieldErrors = { name?: string; token?: string; form?: string };

function connectErrors(err: unknown): FieldErrors {
  if (!(err instanceof ApiError)) return { form: errorText(err) };
  switch (err.code) {
    case "invalid_token":
    case "hetzner_invalid_token":
      return {
        token:
          "Hetzner rejected this token. Check that it is copied completely, belongs to the project and is not revoked.",
      };
    case "name_taken":
      return { name: "A project with this name exists already. Choose another name." };
    case "hetzner_unavailable":
      return {
        form: "The Hetzner Cloud API is not reachable right now, so the token could not be checked. Try again in a minute.",
      };
    case "hetzner_rate_limited":
      return {
        form: "Hetzner's rate limit for this token is used up. Wait until it resets (at most an hour) and try again.",
      };
    case "bad_request":
      if (err.message.includes("`token`")) return { token: "Paste a Hetzner Cloud API token." };
      if (err.message.includes("`name`")) return { name: "Use 1–200 characters." };
      return { form: err.message };
    default:
      return { form: errorText(err) };
  }
}

function ConnectDialog({ open, onClose }: { open: boolean; onClose: () => void }) {
  const [name, setName] = useState("");
  const [token, setToken] = useState("");
  const [errors, setErrors] = useState<FieldErrors>({});
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const notify = useToast();

  const connect = useMutation({
    mutationFn: (body: Schemas["CreateProject"]) => unwrap(api.POST("/api/v1/projects", { body })),
    onSuccess: (created) => {
      void queryClient.invalidateQueries({ queryKey: ["projects"] });
      if (created.sync) {
        const summary = syncSummary(created.sync);
        notify("success", `Connected ${created.project.name}: ${summary.title}`, summary.detail);
      } else {
        notify(
          "warning",
          `Connected ${created.project.name}`,
          "The first inventory sync failed; the scheduler retries it.",
        );
      }
      close();
      void navigate(`/projects/${created.project.id}`);
    },
    onError: (err) => {
      setErrors(connectErrors(err));
      // Drop the settled mutation: its variables hold the plaintext token.
      connect.reset();
    },
    // Do not keep the finished mutation (and the token in its variables) in the cache.
    gcTime: 0,
  });

  function close() {
    setName("");
    setToken("");
    setErrors({});
    connect.reset();
    onClose();
  }

  function submit(e: FormEvent) {
    e.preventDefault();
    const body = { name: name.trim(), token: token.trim() };
    // The token never stays in component state once it is sent.
    setToken("");
    setErrors({});
    connect.mutate(body);
  }

  return (
    <Dialog open={open} onClose={close} title="Connect project">
      <form className="form" onSubmit={submit}>
        <TextField
          label="Name"
          required
          maxLength={200}
          placeholder="production"
          value={name}
          error={errors.name}
          onChange={(e) => setName(e.target.value)}
        />
        <TextField
          label="API token"
          type="password"
          autoComplete="off"
          spellCheck={false}
          required
          className="input mono"
          value={token}
          error={errors.token}
          hint={
            <>
              A <strong>Read</strong> token is enough and recommended: this tool never changes
              anything. Create one in the Hetzner Console under your project &gt; Security &gt; API
              tokens.
            </>
          }
          onChange={(e) => setToken(e.target.value)}
        />
        {errors.form && <ErrorNote>{errors.form}</ErrorNote>}
        <div className="dialog-actions">
          <Button variant="ghost" onClick={close}>
            Cancel
          </Button>
          <Button type="submit" variant="primary" disabled={connect.isPending}>
            {connect.isPending ? "Connecting…" : "Connect project"}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}

// ---------------------------------------------------------------------------------------------
// Detail

export function ProjectDetailPage() {
  const { id = "" } = useParams();
  const navigate = useNavigate();
  const notify = useToast();
  const queryClient = useQueryClient();
  const [deleteOpen, setDeleteOpen] = useState(false);

  const project = useQuery({
    queryKey: ["project", id],
    queryFn: async () =>
      (await unwrap(api.GET("/api/v1/projects/{id}", { params: { path: { id } } }))).project,
  });

  const sync = useMutation({
    mutationFn: () => unwrap(api.POST("/api/v1/projects/{id}/sync", { params: { path: { id } } })),
    onSuccess: (res) => {
      const summary = syncSummary(res.sync);
      notify("success", `Sync finished: ${summary.title}`, summary.detail);
    },
    onError: (err) => {
      if (err instanceof ApiError && err.code === "sync_in_progress") {
        notify("warning", "A poll cycle is running", "Try again in a few seconds.");
      } else {
        notify("danger", "Sync failed", errorText(err));
      }
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ["project", id] });
      void queryClient.invalidateQueries({ queryKey: ["resources", id] });
      void queryClient.invalidateQueries({ queryKey: ["projects"] });
    },
  });

  const remove = useMutation({
    mutationFn: () => unwrap(api.DELETE("/api/v1/projects/{id}", { params: { path: { id } } })),
    onSuccess: () => {
      const name = project.data?.name ?? "Project";
      setDeleteOpen(false);
      // The detail queries are not removed here: this page is still mounted until the
      // navigation transition commits and would refetch the deleted project. Once the
      // page unmounts they are inactive and garbage-collected.
      void navigate("/projects");
      void queryClient.invalidateQueries({ queryKey: ["projects"] });
      void queryClient.invalidateQueries({ queryKey: ["alerts"] });
      void queryClient.invalidateQueries({ queryKey: ["rules"] });
      notify("success", `Deleted ${name}`);
    },
  });

  const back = (
    <Link to="/projects" className="overline">
      Projects
    </Link>
  );

  if (project.isPending) return <Spinner label="Loading project" />;
  if (project.isError) {
    const missing = project.error instanceof ApiError && project.error.status === 404;
    return (
      <>
        <PageHead title={missing ? "Project not found" : "Project"} eyebrow={back} />
        <ErrorNote>
          {missing
            ? "This project does not exist or was deleted."
            : `Could not load the project: ${errorText(project.error)}`}
        </ErrorNote>
      </>
    );
  }

  const p = project.data;
  return (
    <>
      <PageHead
        eyebrow={back}
        title={p.name}
        description={
          <span className="row" style={{ gap: 8 }}>
            <StatusBadge status={p.status} />
            <span className="mono small">{tokenHint(p.token_hint)}</span>
          </span>
        }
        actions={
          <>
            <Link to={`/alerts?project=${encodeURIComponent(p.id)}`} className="btn btn-ghost">
              <Bell size={16} strokeWidth={1.5} />
              Alerts
            </Link>
            <Button
              variant="danger"
              icon={<Trash2 size={16} strokeWidth={1.5} />}
              onClick={() => {
                remove.reset();
                setDeleteOpen(true);
              }}
            >
              Delete project
            </Button>
            <Button
              variant="primary"
              icon={<RefreshCw size={16} strokeWidth={1.5} />}
              disabled={sync.isPending}
              onClick={() => sync.mutate()}
            >
              {sync.isPending ? "Syncing…" : "Sync now"}
            </Button>
          </>
        }
      />
      {p.status_message && (
        <div
          className={p.status === "active" ? "callout" : "callout callout-warning"}
          role="status"
        >
          <div>{p.status_message}</div>
        </div>
      )}
      <div className="grid">
        <CostCard cost={p.cost_estimate} />
        <ScheduleCard project={p} />
        <BudgetCard project={p} />
      </div>
      <Inventory projectId={p.id} />
      <Dialog open={deleteOpen} onClose={() => setDeleteOpen(false)} title="Delete project">
        <div className="stack">
          <p>
            Delete <strong>{p.name}</strong>? Its alerts, rules, inventory and history are deleted
            here. Nothing changes at Hetzner.
          </p>
          {remove.isError && <ErrorNote>{errorText(remove.error)}</ErrorNote>}
          <div className="dialog-actions">
            <Button variant="ghost" onClick={() => setDeleteOpen(false)}>
              Cancel
            </Button>
            <Button
              variant="danger"
              icon={<Trash2 size={16} strokeWidth={1.5} />}
              disabled={remove.isPending}
              onClick={() => remove.mutate()}
            >
              {remove.isPending ? "Deleting…" : "Delete project"}
            </Button>
          </div>
        </div>
      </Dialog>
    </>
  );
}

function CostCard({ cost }: { cost: Schemas["CostView"] | null }) {
  return (
    <Card title="Cost estimate" eyebrow="Monthly, net">
      {cost ? (
        <>
          <p className="card-title">
            {money(cost.monthly_net, cost.currency)}{" "}
            <span className="small muted">per month, net</span>
          </p>
          <dl className="kv">
            <dt>Estimated on</dt>
            <dd>{cost.day}</dd>
            <dt>Unpriced</dt>
            <dd>
              {cost.unpriced === 0
                ? "None"
                : `${cost.unpriced} ${cost.unpriced === 1 ? "resource" : "resources"} without a price (not included)`}
            </dd>
          </dl>
        </>
      ) : (
        <p className="muted small">Available after the first inventory cycle.</p>
      )}
    </Card>
  );
}

function ScheduleCard({ project: p }: { project: Project }) {
  return (
    <Card title="Schedule" eyebrow="Poll cycles">
      <dl className="kv">
        <dt>Status checks</dt>
        <dd>
          <When at={p.last_fast_at} /> <span className="subtle">· next</span>{" "}
          <When at={p.next_fast_at} />
        </dd>
        <dt>Inventory</dt>
        <dd>
          <When at={p.last_inventory_at} /> <span className="subtle">· next</span>{" "}
          <When at={p.next_inventory_at} />
        </dd>
        <dt>Metrics</dt>
        <dd>
          {p.next_metrics_at || p.last_metrics_at ? (
            <>
              <When at={p.last_metrics_at} /> <span className="subtle">· next</span>{" "}
              <When at={p.next_metrics_at} />
            </>
          ) : (
            <span className="muted">Idle (no metric rule covers a server)</span>
          )}
        </dd>
        <dt>Metrics interval</dt>
        <dd>{p.metrics_interval_s != null ? `every ${duration(p.metrics_interval_s)}` : "—"}</dd>
        <dt>Failures</dt>
        <dd>
          {p.failures === 0 ? (
            "None"
          ) : (
            <Badge tone="warning">{`${p.failures} failed ${p.failures === 1 ? "cycle" : "cycles"} in a row`}</Badge>
          )}
        </dd>
      </dl>
    </Card>
  );
}

function BudgetCard({ project: p }: { project: Project }) {
  const perHour = (n: number | null) => (n == null ? "—" : `${n} requests/h`);
  return (
    <Card title="Rate-limit budget" eyebrow="Hetzner API">
      <dl className="kv">
        <dt>Hetzner limit</dt>
        <dd>{perHour(p.rl_limit)}</dd>
        <dt>Remaining</dt>
        <dd>{p.rl_remaining ?? "—"}</dd>
        <dt>Resets</dt>
        <dd>
          <When at={p.rl_reset_at} />
        </dd>
        <dt>Share allowed for this tool</dt>
        <dd>{`${Math.round(p.budget_share * 100)} %`}</dd>
        <dt>Budget</dt>
        <dd>{perHour(p.budget_h)}</dd>
        <dt>Fixed tiers</dt>
        <dd>{perHour(p.fixed_h)}</dd>
      </dl>
    </Card>
  );
}

function Inventory({ projectId }: { projectId: string }) {
  const [params, setParams] = useSearchParams();
  const raw = params.get("kind");
  const kind: ResourceKind = KINDS.includes(raw as ResourceKind) ? (raw as ResourceKind) : "server";

  const resources = useQuery({
    queryKey: ["resources", projectId, kind],
    queryFn: async () =>
      (
        await unwrap(
          api.GET("/api/v1/projects/{id}/resources", {
            params: { path: { id: projectId }, query: { kind } },
          }),
        )
      ).resources,
  });

  return (
    <Card
      title="Inventory"
      actions={
        <SelectField
          label="Kind"
          value={kind}
          onChange={(e) => {
            const next = new URLSearchParams(params);
            next.set("kind", e.target.value);
            setParams(next, { replace: true });
          }}
        >
          {KINDS.map((k) => (
            <option key={k} value={k}>
              {KIND_LABELS[k]}
            </option>
          ))}
        </SelectField>
      }
    >
      {resources.isPending ? (
        <Spinner label="Loading inventory" />
      ) : resources.isError ? (
        <ErrorNote>Could not load the inventory: {errorText(resources.error)}</ErrorNote>
      ) : resources.data.length === 0 ? (
        <Empty title={`No ${KIND_LABELS[kind].toLowerCase()} in this project`} />
      ) : (
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>Name</th>
                <th>Hetzner ID</th>
                <th className="hide-mobile">Labels</th>
                <th className="hide-mobile">First seen</th>
                <th>Last seen</th>
              </tr>
            </thead>
            <tbody>
              {resources.data.map((r) => (
                <tr key={r.id}>
                  <td className="strong">
                    <span className="row" style={{ gap: 8 }}>
                      {r.name}
                      {r.deleted_at && (
                        <span title={`Deleted at Hetzner ${dateTime(r.deleted_at)}`}>
                          <Badge tone="danger">deleted</Badge>
                        </span>
                      )}
                    </span>
                  </td>
                  <td className="mono">{r.hetzner_id}</td>
                  <td className="hide-mobile">
                    <Labels labels={r.labels} />
                  </td>
                  <td className="hide-mobile">
                    <When at={r.first_seen_at} />
                  </td>
                  <td>
                    <When at={r.last_seen_at} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Card>
  );
}

function Labels({ labels }: { labels: Record<string, string> }) {
  const entries = Object.entries(labels).sort(([a], [b]) => a.localeCompare(b));
  if (entries.length === 0) return <span className="subtle">—</span>;
  return (
    <span className="row" style={{ gap: 4 }}>
      {entries.map(([k, v]) => (
        <Badge key={k} tone="neutral">
          <span className="mono">{v ? `${k}=${v}` : k}</span>
        </Badge>
      ))}
    </span>
  );
}
