import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppRoutes } from "../App";
import { mockFetch, PROFILE, renderApp } from "../test/utils";

const PROJECT = { id: "p1", name: "Production" };
const ALERT_ID = "a1";

function alert(overrides: Record<string, unknown> = {}) {
  return {
    id: ALERT_ID,
    project_id: PROJECT.id,
    rule_id: "r1",
    rule_name: "Server not running",
    check_type: "server_status",
    state: "firing",
    severity: "critical",
    message: "web-1 is off",
    pending_since: "2026-11-13T09:00:00Z",
    fired_at: "2026-11-13T09:01:00Z",
    resolved_at: null,
    updated_at: "2026-11-13T09:01:00Z",
    acked_at: null,
    acked_by: null,
    hetzner_id: 42,
    last_notified_at: "2026-11-13T09:01:05Z",
    resource_id: "res1",
    resource_kind: "server",
    resource_name: "web-1",
    value: null,
    ...overrides,
  };
}

function list(alerts: unknown[], page = 1, lastPage = 1) {
  return {
    alerts,
    meta: {
      pagination: {
        page,
        per_page: 25,
        last_page: lastPage,
        total_entries: alerts.length,
        next_page: page < lastPage ? page + 1 : null,
        previous_page: page > 1 ? page - 1 : null,
      },
    },
  };
}

afterEach(() => vi.unstubAllGlobals());

describe("AlertsPage", () => {
  it("lists alerts and sends the state filter as query", async () => {
    const calls = mockFetch(({ method, path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (method === "GET" && path === "/api/v1/projects") return { body: { projects: [PROJECT] } };
      if (method === "GET" && path.startsWith("/api/v1/alerts?")) {
        const state = new URLSearchParams(path.split("?")[1]).get("state");
        return {
          body: list(
            state === "resolved"
              ? []
              : [
                  alert(),
                  alert({
                    id: "a2",
                    rule_name: "CPU high",
                    state: "pending",
                    fired_at: null,
                    acked_at: "2026-11-13T09:05:00Z",
                  }),
                ],
          ),
        };
      }
      return undefined;
    });
    renderApp(<AppRoutes />, "/alerts");

    const row = (await screen.findByRole("link", { name: "Server not running" })).closest("tr")!;
    expect(row).toHaveTextContent("web-1 is off");
    expect(row).toHaveTextContent("Critical");
    expect(row).toHaveTextContent("Firing");
    expect(row).toHaveTextContent("web-1 · Server");
    expect(await within(row).findByText("Production")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "CPU high" })).toHaveAttribute("href", "/alerts/a2");
    expect(screen.getByLabelText("Acknowledged")).toBeInTheDocument();

    const first = calls.find((c) => c.path.startsWith("/api/v1/alerts?"));
    expect(first?.path).toContain("state=open");

    await userEvent.selectOptions(screen.getByLabelText("State"), "resolved");
    expect(await screen.findByText("No resolved alerts")).toBeInTheDocument();
    expect(
      calls.some((c) => c.path.startsWith("/api/v1/alerts?") && c.path.includes("state=resolved")),
    ).toBe(true);

    await userEvent.selectOptions(screen.getByLabelText("Project"), "p1");
    await waitFor(() =>
      expect(
        calls.some((c) => c.path.includes("state=resolved") && c.path.includes("project_id=p1")),
      ).toBe(true),
    );
  });

  it("pages through the results", async () => {
    const calls = mockFetch(({ path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (path === "/api/v1/projects") return { body: { projects: [PROJECT] } };
      if (path.startsWith("/api/v1/alerts?")) {
        const page = Number(new URLSearchParams(path.split("?")[1]).get("page") ?? "1");
        return { body: list([alert({ rule_name: `Rule on page ${page}` })], page, 2) };
      }
      return undefined;
    });
    renderApp(<AppRoutes />, "/alerts");

    await screen.findByText("Rule on page 1");
    expect(screen.getByRole("button", { name: "Previous" })).toBeDisabled();
    await userEvent.click(screen.getByRole("button", { name: "Next" }));
    await screen.findByText("Rule on page 2");
    expect(calls.some((c) => c.path.includes("page=2"))).toBe(true);
  });

  it("goes back to the last page when the page is past the end", async () => {
    const calls = mockFetch(({ path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (path === "/api/v1/projects") return { body: { projects: [PROJECT] } };
      if (path.startsWith("/api/v1/alerts?")) {
        const page = Number(new URLSearchParams(path.split("?")[1]).get("page") ?? "1");
        // One page of alerts; anything beyond it comes back empty, as the server does.
        const body = list(page === 1 ? [alert()] : [], page, 1);
        body.meta.pagination.total_entries = 1;
        return { body };
      }
      return undefined;
    });
    renderApp(<AppRoutes />, "/alerts?page=3");

    expect(await screen.findByRole("link", { name: "Server not running" })).toBeInTheDocument();
    const pages = calls
      .filter((c) => c.path.startsWith("/api/v1/alerts?"))
      .map((c) => new URLSearchParams(c.path.split("?")[1]).get("page"));
    expect(pages[0]).toBe("3");
    expect(pages.at(-1)).toBe("1");
    expect(screen.queryByText("No open alerts")).not.toBeInTheDocument();
  });
});

describe("AlertDetailPage", () => {
  it("acknowledges an open alert with the CSRF header", async () => {
    let acked = false;
    const detail = () => ({
      alert: alert(acked ? { acked_at: "2026-11-13T09:10:00Z", acked_by: PROFILE.user.id } : {}),
      events: [
        {
          id: 1,
          type: "pending",
          at: "2026-11-13T09:00:00Z",
          data: { message: "web-1 is off", value: null },
        },
        {
          id: 2,
          type: "fired",
          at: "2026-11-13T09:01:00Z",
          data: { message: "web-1 is off", value: null },
        },
        ...(acked
          ? [{ id: 3, type: "acked", at: "2026-11-13T09:10:00Z", data: { by: PROFILE.user.id } }]
          : []),
      ],
      notifications: [
        {
          id: 7,
          alert_event_id: 2,
          channel_id: "c1",
          channel_name: "Ops Discord",
          device: null,
          status: "failed",
          attempts: 3,
          next_attempt_at: "2026-11-13T09:02:00Z",
          finished_at: "2026-11-13T09:05:00Z",
          last_error: "HTTP 500",
        },
      ],
    });
    const calls = mockFetch(({ method, path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (path === "/api/v1/projects") return { body: { projects: [PROJECT] } };
      if (method === "GET" && path === `/api/v1/alerts/${ALERT_ID}`) return { body: detail() };
      if (method === "POST" && path === `/api/v1/alerts/${ALERT_ID}/ack`) {
        acked = true;
        return { body: { alert: detail().alert } };
      }
      return undefined;
    });
    renderApp(<AppRoutes />, `/alerts/${ALERT_ID}`);

    expect(await screen.findByRole("heading", { name: "Server not running" })).toBeInTheDocument();
    expect(screen.getByText("Ops Discord")).toBeInTheDocument();
    expect(screen.getByText("HTTP 500")).toBeInTheDocument();
    expect(await screen.findByRole("link", { name: "Production" })).toHaveAttribute(
      "href",
      "/projects/p1",
    );

    await userEvent.click(screen.getByRole("button", { name: "Acknowledge" }));

    const post = await waitFor(() => {
      const call = calls.find((c) => c.method === "POST" && c.path.endsWith("/ack"));
      expect(call).toBeDefined();
      return call!;
    });
    expect(post.headers.get("X-CSRF-Token")).toBe("csrf-1");
    await waitFor(() =>
      expect(screen.queryByRole("button", { name: /Acknowledg/ })).not.toBeInTheDocument(),
    );
    expect(screen.getAllByText("Acknowledged").length).toBeGreaterThan(0);
    expect(await screen.findByText("By you")).toBeInTheDocument();
    expect(screen.getByText("Alert acknowledged")).toBeInTheDocument();
  });

  it("shows a toast when the alert resolved in the meantime", async () => {
    mockFetch(({ method, path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (path === "/api/v1/projects") return { body: { projects: [PROJECT] } };
      if (method === "GET" && path === `/api/v1/alerts/${ALERT_ID}`) {
        return { body: { alert: alert(), events: [], notifications: [] } };
      }
      if (method === "POST") {
        return {
          status: 409,
          body: { error: { code: "alert_resolved", message: "alert is already resolved" } },
        };
      }
      return undefined;
    });
    renderApp(<AppRoutes />, `/alerts/${ALERT_ID}`);

    await userEvent.click(await screen.findByRole("button", { name: "Acknowledge" }));
    expect(await screen.findByText("Alert already resolved")).toBeInTheDocument();
  });

  it("shows not found for an unknown alert", async () => {
    mockFetch(({ path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (path === "/api/v1/projects") return { body: { projects: [PROJECT] } };
      return undefined;
    });
    renderApp(<AppRoutes />, "/alerts/unknown");

    expect(await screen.findByText("Alert not found")).toBeInTheDocument();
    expect(screen.getByRole("link", { name: "Back to alerts" })).toHaveAttribute("href", "/alerts");
  });
});
