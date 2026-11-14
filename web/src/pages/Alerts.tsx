// Alert list (filters in the URL, paginated, auto-refresh) and alert detail with acknowledge.
import { keepPreviousData, useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { ArrowLeft, Check, ChevronLeft, ChevronRight, SearchX } from "lucide-react";
import { useEffect } from "react";
import { Link, useNavigate, useParams, useSearchParams } from "react-router";
import { api, ApiError, errorText, unwrap, type Schemas } from "../api/client";
import { useProfile } from "../api/session";
import {
  alertStateTone,
  CHECK_LABELS,
  dateTime,
  relTime,
  severityTone,
  type AlertState,
  type ResourceKind,
} from "../format";
import {
  Badge,
  Button,
  Card,
  Empty,
  ErrorNote,
  PageHead,
  SelectField,
  Spinner,
} from "../ui/components";
import { useToast } from "../ui/toast";

type AlertFilter = Schemas["AlertFilter"];
type AlertView = Schemas["AlertView"];
type EventView = Schemas["EventView"];
type JobView = Schemas["JobView"];

const FILTERS: { value: AlertFilter; label: string }[] = [
  { value: "open", label: "Open" },
  { value: "pending", label: "Pending" },
  { value: "firing", label: "Firing" },
  { value: "resolved", label: "Resolved" },
];

const KIND_SINGULAR: Record<ResourceKind, string> = {
  server: "Server",
  load_balancer: "Load balancer",
  volume: "Volume",
  certificate: "Certificate",
  floating_ip: "Floating IP",
  primary_ip: "Primary IP",
};

const STATE_LABELS: Record<AlertState, string> = {
  pending: "Pending",
  firing: "Firing",
  resolved: "Resolved",
};

const SEVERITY_LABELS: Record<Schemas["Severity"], string> = {
  info: "Info",
  warning: "Warning",
  critical: "Critical",
};

const EVENT_LABELS: Record<Schemas["AlertEventType"], string> = {
  pending: "Pending",
  fired: "Fired",
  resolved: "Resolved",
  acked: "Acknowledged",
  renotified: "Re-notified",
  suppressed: "Notification silenced",
};

const JOB_TONES: Record<Schemas["JobStatus"], "info" | "success" | "danger"> = {
  queued: "info",
  sent: "success",
  failed: "danger",
};

const JOB_LABELS: Record<Schemas["JobStatus"], string> = {
  queued: "Queued",
  sent: "Sent",
  failed: "Failed",
};

function isFilter(value: string | null): value is AlertFilter {
  return FILTERS.some((f) => f.value === value);
}

function resourceLabel(alert: AlertView): string | null {
  if (!alert.resource_name && !alert.resource_kind) return null;
  const kind = alert.resource_kind ? KIND_SINGULAR[alert.resource_kind] : null;
  return [alert.resource_name, kind].filter(Boolean).join(" · ");
}

function formatValue(value: number | null): string {
  if (value === null) return "—";
  return value.toLocaleString(undefined, { maximumFractionDigits: 2 });
}

function useProjects() {
  return useQuery({
    queryKey: ["projects"],
    queryFn: () => unwrap(api.GET("/api/v1/projects")),
  });
}

export function AlertsPage() {
  const [params, setParams] = useSearchParams();
  const navigate = useNavigate();
  const rawState = params.get("state");
  const state: AlertFilter = isFilter(rawState) ? rawState : "open";
  const projectId = params.get("project") ?? "";
  const page = Math.max(1, Number.parseInt(params.get("page") ?? "1", 10) || 1);

  const projects = useProjects();
  const alerts = useQuery({
    queryKey: ["alerts", { state, projectId, page }],
    queryFn: () =>
      unwrap(
        api.GET("/api/v1/alerts", {
          params: {
            query: { state, page, ...(projectId ? { project_id: projectId } : {}) },
          },
        }),
      ),
    refetchInterval: 30_000,
    placeholderData: keepPreviousData,
  });

  const projectNames = new Map((projects.data?.projects ?? []).map((p) => [p.id, p.name]));

  function update(changes: Record<string, string | null>) {
    const next = new URLSearchParams(params);
    for (const [key, value] of Object.entries(changes)) {
      if (value === null || value === "") next.delete(key);
      else next.set(key, value);
    }
    setParams(next, { replace: true });
  }

  const filterLabel = FILTERS.find((f) => f.value === state)?.label.toLowerCase() ?? state;
  const pagination = alerts.data?.meta.pagination;
  const now = new Date();
  // A page past the end (old bookmark, or alerts resolved since the last refetch): the server
  // does not clamp it and returns an empty list, so go back to the last page that has alerts.
  const lastPage = pagination ? Math.max(1, pagination.last_page) : 1;
  const pastEnd =
    !!alerts.data &&
    !alerts.isPlaceholderData &&
    alerts.data.alerts.length === 0 &&
    page > lastPage;

  useEffect(() => {
    if (!pastEnd) return;
    setParams(
      (prev) => {
        const next = new URLSearchParams(prev);
        if (lastPage > 1) next.set("page", String(lastPage));
        else next.delete("page");
        return next;
      },
      { replace: true },
    );
  }, [pastEnd, lastPage, setParams]);

  return (
    <>
      <PageHead
        title="Alerts"
        description="What the checks found in your Hetzner Cloud projects. Updates every 30 seconds."
      />
      <div className="form-row">
        <SelectField
          label="State"
          value={state}
          onChange={(e) =>
            update({ state: e.target.value === "open" ? null : e.target.value, page: null })
          }
        >
          {FILTERS.map((f) => (
            <option key={f.value} value={f.value}>
              {f.label}
            </option>
          ))}
        </SelectField>
        <SelectField
          label="Project"
          value={projectId}
          disabled={projects.isPending}
          onChange={(e) => update({ project: e.target.value, page: null })}
        >
          <option value="">All projects</option>
          {projects.data?.projects.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </SelectField>
      </div>

      {alerts.isPending ? (
        <Spinner label="Loading alerts" />
      ) : alerts.isError ? (
        <ErrorNote>Could not load alerts: {errorText(alerts.error)}</ErrorNote>
      ) : pastEnd ? (
        <Empty title="This page is empty">
          <Button size="sm" variant="outline" onClick={() => update({ page: null })}>
            Go to first page
          </Button>
        </Empty>
      ) : alerts.data.alerts.length === 0 ? (
        <Empty title={`No ${filterLabel} alerts`}>
          <p className="small">
            {page > 1
              ? "There are no alerts on this page."
              : projectId
                ? "Nothing matches this filter in the selected project."
                : state === "open"
                  ? "All checks are passing."
                  : "Nothing matches this filter."}
          </p>
        </Empty>
      ) : (
        <>
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Severity</th>
                  <th>State</th>
                  <th>Alert</th>
                  <th className="hide-mobile">Resource</th>
                  <th className="hide-mobile">Project</th>
                  <th>Since</th>
                  <th>Acked</th>
                </tr>
              </thead>
              <tbody>
                {alerts.data.alerts.map((a) => {
                  const since = a.fired_at ?? a.pending_since;
                  return (
                    <tr
                      key={a.id}
                      style={{ cursor: "pointer" }}
                      onClick={() => navigate(`/alerts/${a.id}`)}
                    >
                      <td>
                        <Badge tone={severityTone(a.severity)}>{SEVERITY_LABELS[a.severity]}</Badge>
                      </td>
                      <td>
                        <Badge tone={alertStateTone(a.state)} dot>
                          {STATE_LABELS[a.state]}
                        </Badge>
                      </td>
                      <td style={{ minWidth: 200 }}>
                        <Link
                          className="strong"
                          to={`/alerts/${a.id}`}
                          onClick={(e) => e.stopPropagation()}
                        >
                          {a.rule_name}
                        </Link>
                        <div className="muted small">{a.message}</div>
                      </td>
                      <td className="hide-mobile">
                        {resourceLabel(a) ?? <span className="subtle">—</span>}
                      </td>
                      <td className="hide-mobile">
                        {projectNames.get(a.project_id) ?? <span className="subtle">—</span>}
                      </td>
                      <td style={{ whiteSpace: "nowrap" }}>
                        <span title={dateTime(since)}>{relTime(since, now)}</span>
                      </td>
                      <td>
                        {a.acked_at ? (
                          <span title={`Acknowledged ${dateTime(a.acked_at)}`}>
                            <Check size={16} strokeWidth={1.5} aria-label="Acknowledged" />
                          </span>
                        ) : (
                          <span className="subtle">—</span>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
          {pagination && pagination.last_page > 1 && (
            <nav className="pagination" aria-label="Pagination">
              <span className="small muted">
                Page {pagination.page} of {pagination.last_page} · {pagination.total_entries} alerts
              </span>
              <Button
                size="sm"
                variant="outline"
                icon={<ChevronLeft size={16} strokeWidth={1.5} />}
                disabled={pagination.previous_page === null}
                onClick={() => update({ page: String(pagination.previous_page) })}
              >
                Previous
              </Button>
              <Button
                size="sm"
                variant="outline"
                disabled={pagination.next_page === null}
                onClick={() => update({ page: String(pagination.next_page) })}
              >
                Next
                <ChevronRight size={16} strokeWidth={1.5} />
              </Button>
            </nav>
          )}
        </>
      )}
    </>
  );
}

/** One line of text describing an event's `data`. */
function eventSummary(event: EventView, userId: string): string | null {
  const data = (event.data ?? {}) as Record<string, unknown>;
  switch (event.type) {
    case "acked":
      return data.by === userId ? "By you" : "By another user";
    case "suppressed": {
      const what = typeof data.event === "string" ? data.event : null;
      const label = what && what in EVENT_LABELS ? EVENT_LABELS[what as EventView["type"]] : null;
      return label
        ? `The "${label.toLowerCase()}" notification fell under a silence`
        : "Fell under a silence";
    }
    default: {
      const parts: string[] = [];
      if (typeof data.message === "string" && data.message) parts.push(data.message);
      if (typeof data.value === "number") parts.push(`value ${formatValue(data.value)}`);
      return parts.length ? parts.join(" · ") : null;
    }
  }
}

function jobWhen(job: JobView): string {
  if (job.finished_at) return dateTime(job.finished_at);
  return `Next attempt ${relTime(job.next_attempt_at)}`;
}

export function AlertDetailPage() {
  const { id = "" } = useParams();
  const profile = useProfile();
  const queryClient = useQueryClient();
  const notify = useToast();
  const projects = useProjects();

  const detail = useQuery({
    queryKey: ["alert", id],
    queryFn: () => unwrap(api.GET("/api/v1/alerts/{id}", { params: { path: { id } } })),
    refetchInterval: 30_000,
  });

  const ack = useMutation({
    mutationFn: () => unwrap(api.POST("/api/v1/alerts/{id}/ack", { params: { path: { id } } })),
    onSuccess: (data) => {
      queryClient.setQueryData<Schemas["AlertDetail"]>(["alert", id], (old) =>
        old ? { ...old, alert: data.alert } : old,
      );
      notify("success", "Alert acknowledged", "Re-notifications stop; the resolve is still sent.");
    },
    onError: (error) => {
      if (error instanceof ApiError && error.code === "alert_resolved") {
        notify("warning", "Alert already resolved", "There is nothing left to acknowledge.");
      } else {
        notify("danger", "Could not acknowledge the alert", errorText(error));
      }
    },
    onSettled: () => {
      void queryClient.invalidateQueries({ queryKey: ["alert", id] });
      void queryClient.invalidateQueries({ queryKey: ["alerts"] });
    },
  });

  const back = (
    <Link to="/alerts" className="small row" style={{ gap: 6 }}>
      <ArrowLeft size={16} strokeWidth={1.5} aria-hidden="true" />
      Back to alerts
    </Link>
  );

  if (detail.isPending) return <Spinner label="Loading alert" />;
  if (detail.isError) {
    if (detail.error instanceof ApiError && detail.error.status === 404) {
      return (
        <Empty title="Alert not found" icon={<SearchX size={24} strokeWidth={1.5} />}>
          <p className="small">It may belong to a deleted project or rule.</p>
          {back}
        </Empty>
      );
    }
    return (
      <>
        {back}
        <ErrorNote>Could not load the alert: {errorText(detail.error)}</ErrorNote>
      </>
    );
  }

  const { alert, events, notifications } = detail.data;
  const projectName =
    projects.data?.projects.find((p) => p.id === alert.project_id)?.name ?? "Open project";
  const canAck = alert.state !== "resolved" && !alert.acked_at;
  const eventTypes = new Map(events.map((e) => [e.id, e.type]));
  const resource = resourceLabel(alert);

  return (
    <>
      {back}
      <PageHead
        eyebrow={CHECK_LABELS[alert.check_type]}
        title={alert.rule_name}
        description={alert.message}
        actions={
          canAck && (
            <Button
              variant="primary"
              icon={<Check size={16} strokeWidth={1.5} />}
              disabled={ack.isPending}
              onClick={() => ack.mutate()}
            >
              {ack.isPending ? "Acknowledging…" : "Acknowledge"}
            </Button>
          )
        }
      />
      <div className="row" style={{ gap: 8 }}>
        <Badge tone={severityTone(alert.severity)}>{SEVERITY_LABELS[alert.severity]}</Badge>
        <Badge tone={alertStateTone(alert.state)} dot>
          {STATE_LABELS[alert.state]}
        </Badge>
        {alert.acked_at && <Badge tone="neutral">Acknowledged</Badge>}
      </div>

      <Card title="Details">
        <dl className="kv">
          <dt>Rule</dt>
          <dd>{alert.rule_name}</dd>
          <dt>Check</dt>
          <dd>{CHECK_LABELS[alert.check_type]}</dd>
          <dt>Project</dt>
          <dd>
            <Link to={`/projects/${alert.project_id}`}>{projectName}</Link>
          </dd>
          <dt>Resource</dt>
          <dd>
            {resource ?? "—"}
            {alert.hetzner_id !== null && (
              <span className="muted mono"> (Hetzner ID {alert.hetzner_id})</span>
            )}
          </dd>
          <dt>Value</dt>
          <dd>{formatValue(alert.value)}</dd>
          <dt>Pending since</dt>
          <dd>{dateTime(alert.pending_since)}</dd>
          <dt>Fired</dt>
          <dd>{dateTime(alert.fired_at)}</dd>
          <dt>Resolved</dt>
          <dd>{dateTime(alert.resolved_at)}</dd>
          <dt>Acknowledged</dt>
          <dd>
            {alert.acked_at
              ? `${dateTime(alert.acked_at)}, ${alert.acked_by === profile.user.id ? "by you" : "by another user"}`
              : "—"}
          </dd>
          <dt>Last notified</dt>
          <dd>{dateTime(alert.last_notified_at)}</dd>
        </dl>
      </Card>

      <Card title="History">
        {events.length === 0 ? (
          <p className="small muted">No events recorded.</p>
        ) : (
          <ol className="timeline">
            {events.map((e) => {
              const summary = eventSummary(e, profile.user.id);
              return (
                <li key={e.id}>
                  <span className="muted" style={{ whiteSpace: "nowrap" }} title={relTime(e.at)}>
                    {dateTime(e.at)}
                  </span>
                  <span className="strong">{EVENT_LABELS[e.type]}</span>
                  {summary && <span className="muted">{summary}</span>}
                </li>
              );
            })}
          </ol>
        )}
      </Card>

      <Card title="Notifications">
        {notifications.length === 0 ? (
          <p className="small muted">
            No notifications. No channel matched this alert, or it has not fired yet.
          </p>
        ) : (
          <div className="table-wrap">
            <table className="table">
              <thead>
                <tr>
                  <th>Channel</th>
                  <th className="hide-mobile">Event</th>
                  <th>Status</th>
                  <th className="hide-mobile">Attempts</th>
                  <th>When</th>
                  <th className="hide-mobile">Last error</th>
                </tr>
              </thead>
              <tbody>
                {notifications.map((n) => {
                  const type = eventTypes.get(n.alert_event_id);
                  return (
                    <tr key={n.id}>
                      <td className="strong">
                        {n.channel_name}
                        {n.device && <span className="muted"> · {n.device}</span>}
                      </td>
                      <td className="hide-mobile">{type ? EVENT_LABELS[type] : "—"}</td>
                      <td>
                        <Badge tone={JOB_TONES[n.status]} dot>
                          {JOB_LABELS[n.status]}
                        </Badge>
                      </td>
                      <td className="hide-mobile">{n.attempts}</td>
                      <td style={{ whiteSpace: "nowrap" }}>{jobWhen(n)}</td>
                      <td className="hide-mobile">
                        {n.last_error ? (
                          <span className="small mono">{n.last_error}</span>
                        ) : (
                          <span className="subtle">—</span>
                        )}
                      </td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
          </div>
        )}
      </Card>
    </>
  );
}
