import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppRoutes } from "../App";
import { mockFetch, PROFILE, renderApp } from "../test/utils";

const ID = "11111111-1111-4111-8111-111111111111";

const PROJECT = {
  id: ID,
  name: "production",
  status: "active",
  status_message: null,
  token_hint: "abcd",
  failures: 0,
  budget_share: 0.5,
  budget_h: 1800,
  fixed_h: 120,
  rl_limit: 3600,
  rl_remaining: 3500,
  rl_reset_at: "2026-11-13T11:00:00Z",
  created_at: "2026-11-01T10:00:00Z",
  last_fast_at: "2026-11-13T10:00:00Z",
  next_fast_at: "2026-11-13T10:01:00Z",
  last_inventory_at: "2026-11-13T09:50:00Z",
  next_inventory_at: "2026-11-13T10:50:00Z",
  last_metrics_at: null,
  next_metrics_at: null,
  metrics_interval_s: null,
};

const SERVER = {
  id: "22222222-2222-4222-8222-222222222222",
  kind: "server",
  hetzner_id: 4711,
  name: "web-1",
  labels: { env: "prod" },
  first_seen_at: "2026-11-01T10:00:00Z",
  last_seen_at: "2026-11-13T09:50:00Z",
  deleted_at: null,
};

const OLD_SERVER = {
  ...SERVER,
  id: "33333333-3333-4333-8333-333333333333",
  hetzner_id: 42,
  name: "old-db",
  labels: {},
  deleted_at: "2026-11-10T08:00:00Z",
};

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ProjectsPage", () => {
  it("lists projects with status, token hint and budget", async () => {
    mockFetch(({ method, path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (method === "GET" && path === "/api/v1/projects") return { body: { projects: [PROJECT] } };
    });
    renderApp(<AppRoutes />, "/projects");
    const card = (await screen.findByText("production")).closest("a")!;
    expect(card).toHaveAttribute("href", `/projects/${ID}`);
    expect(within(card).getByText("Active")).toBeInTheDocument();
    expect(within(card).getByText("…abcd")).toBeInTheDocument();
    expect(within(card).getByText("1800 of 3600 requests/h")).toBeInTheDocument();
  });

  it("explains the first step when no project is connected", async () => {
    mockFetch(({ path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (path === "/api/v1/projects") return { body: { projects: [] } };
    });
    renderApp(<AppRoutes />, "/projects");
    expect(await screen.findByText("No project connected yet")).toBeInTheDocument();
  });

  it("connects a project, clears the token and opens the detail page", async () => {
    const user = userEvent.setup();
    const calls = mockFetch(({ method, path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (method === "GET" && path === "/api/v1/projects") return { body: { projects: [] } };
      if (method === "POST" && path === "/api/v1/projects") {
        return {
          status: 201,
          body: {
            project: PROJECT,
            sync: {
              server: { seen: 2, new: 2, deleted: 0 },
              volume: { seen: 1, new: 1, deleted: 0 },
            },
          },
        };
      }
      if (path === `/api/v1/projects/${ID}`) {
        return { body: { project: { ...PROJECT, cost_estimate: null } } };
      }
      if (path.startsWith(`/api/v1/projects/${ID}/resources`)) return { body: { resources: [] } };
    });
    renderApp(<AppRoutes />, "/projects");
    await user.click(await screen.findByRole("button", { name: "Connect project" }));
    await user.type(screen.getByLabelText("Name"), "production");
    await user.type(screen.getByLabelText("API token"), "secret-token-abcd");
    await user.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Connect project" }),
    );

    await waitFor(() =>
      expect(calls.some((c) => c.method === "POST" && c.path === "/api/v1/projects")).toBe(true),
    );
    const post = calls.find((c) => c.method === "POST" && c.path === "/api/v1/projects")!;
    expect(post.body).toEqual({ name: "production", token: "secret-token-abcd" });
    expect(post.headers.get("X-CSRF-Token")).toBe("csrf-1");

    expect(
      await screen.findByRole("heading", { name: "production", level: 1 }),
    ).toBeInTheDocument();
    expect(
      await screen.findByText("Connected production: 3 resources found (3 new)"),
    ).toBeInTheDocument();
    expect(screen.getByText("Servers: 2 · Volumes: 1")).toBeInTheDocument();
  });

  it("shows a field-level message when Hetzner rejects the token", async () => {
    const user = userEvent.setup();
    mockFetch(({ method, path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (method === "GET" && path === "/api/v1/projects") return { body: { projects: [] } };
      if (method === "POST" && path === "/api/v1/projects") {
        return {
          status: 422,
          body: { error: { code: "invalid_token", message: "Hetzner rejected the token" } },
        };
      }
    });
    renderApp(<AppRoutes />, "/projects");
    await user.click(await screen.findByRole("button", { name: "Connect project" }));
    await user.type(screen.getByLabelText("Name"), "production");
    const token = screen.getByLabelText("API token");
    await user.type(token, "wrong-token");
    await user.click(
      within(screen.getByRole("dialog")).getByRole("button", { name: "Connect project" }),
    );

    expect(await screen.findByText(/Hetzner rejected this token/)).toBeInTheDocument();
    expect(token).toHaveAttribute("aria-invalid", "true");
    expect(token).toHaveValue("");
    expect(screen.getByLabelText("Name")).toHaveValue("production");
  });
});

describe("ProjectDetailPage", () => {
  function detailHandler(cost: unknown) {
    return mockFetch(({ method, path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (method === "GET" && path === `/api/v1/projects/${ID}`) {
        return { body: { project: { ...PROJECT, cost_estimate: cost } } };
      }
      if (method === "GET" && path === `/api/v1/projects/${ID}/resources?kind=server`) {
        return { body: { resources: [SERVER, OLD_SERVER] } };
      }
      if (method === "GET" && path === `/api/v1/projects/${ID}/resources?kind=volume`) {
        return { body: { resources: [] } };
      }
      if (method === "POST" && path === `/api/v1/projects/${ID}/sync`) {
        return {
          body: { project: PROJECT, sync: { server: { seen: 2, new: 0, deleted: 1 } } },
        };
      }
      if (method === "DELETE" && path === `/api/v1/projects/${ID}`) return { status: 204 };
      if (method === "GET" && path === "/api/v1/projects") return { body: { projects: [] } };
    });
  }

  it("shows the cost estimate, budget and inventory", async () => {
    const user = userEvent.setup();
    detailHandler({
      day: "2026-11-13",
      monthly_net: 42.5,
      currency: "EUR",
      unpriced: 2,
      at: "2026-11-13T00:10:00Z",
    });
    renderApp(<AppRoutes />, `/projects/${ID}`);

    expect(
      await screen.findByRole("heading", { name: "production", level: 1 }),
    ).toBeInTheDocument();
    const money = new Intl.NumberFormat(undefined, { style: "currency", currency: "EUR" }).format(
      42.5,
    );
    expect(screen.getByText(money)).toBeInTheDocument();
    expect(screen.getByText("2 resources without a price (not included)")).toBeInTheDocument();
    expect(screen.getByText("50 %")).toBeInTheDocument();
    expect(screen.getByText("1800 requests/h")).toBeInTheDocument();
    expect(within(screen.getByRole("main")).getByRole("link", { name: "Alerts" })).toHaveAttribute(
      "href",
      `/alerts?project=${ID}`,
    );

    expect(await screen.findByText("web-1")).toBeInTheDocument();
    expect(screen.getByText("4711")).toBeInTheDocument();
    expect(screen.getByText("env=prod")).toBeInTheDocument();
    expect(screen.getByText("old-db")).toBeInTheDocument();
    expect(screen.getByText("deleted")).toBeInTheDocument();

    await user.selectOptions(screen.getByLabelText("Kind"), "volume");
    expect(await screen.findByText("No volumes in this project")).toBeInTheDocument();
  });

  it("says when the first cost estimate will appear", async () => {
    detailHandler(null);
    renderApp(<AppRoutes />, `/projects/${ID}`);
    expect(
      await screen.findByText("Available after the first inventory cycle."),
    ).toBeInTheDocument();
  });

  it("syncs now and reports the result", async () => {
    const user = userEvent.setup();
    const calls = detailHandler(null);
    renderApp(<AppRoutes />, `/projects/${ID}`);
    await user.click(await screen.findByRole("button", { name: "Sync now" }));
    expect(
      await screen.findByText("Sync finished: 2 resources found (1 deleted)"),
    ).toBeInTheDocument();
    expect(
      calls.filter((c) => c.method === "POST" && c.path === `/api/v1/projects/${ID}/sync`),
    ).toHaveLength(1);
  });

  it("tells the user when a poll cycle is running", async () => {
    const user = userEvent.setup();
    mockFetch(({ method, path }) => {
      if (path === "/api/v1/me") return { body: PROFILE };
      if (method === "GET" && path === `/api/v1/projects/${ID}`) {
        return { body: { project: { ...PROJECT, cost_estimate: null } } };
      }
      if (path.startsWith(`/api/v1/projects/${ID}/resources`)) return { body: { resources: [] } };
      if (method === "POST" && path === `/api/v1/projects/${ID}/sync`) {
        return {
          status: 409,
          body: { error: { code: "sync_in_progress", message: "a poll cycle is running" } },
        };
      }
    });
    renderApp(<AppRoutes />, `/projects/${ID}`);
    await user.click(await screen.findByRole("button", { name: "Sync now" }));
    expect(await screen.findByText("A poll cycle is running")).toBeInTheDocument();
  });

  it("deletes the project after confirmation", async () => {
    const user = userEvent.setup();
    const calls = detailHandler(null);
    renderApp(<AppRoutes />, `/projects/${ID}`);
    await user.click(await screen.findByRole("button", { name: "Delete project" }));
    const dialog = screen.getByRole("dialog");
    expect(
      within(dialog).getByText(/alerts, rules, inventory and history are deleted/),
    ).toBeInTheDocument();
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
    await user.click(within(dialog).getByRole("button", { name: "Delete project" }));

    expect(await screen.findByRole("heading", { name: "Projects", level: 1 })).toBeInTheDocument();
    expect(
      calls.filter((c) => c.method === "DELETE" && c.path === `/api/v1/projects/${ID}`),
    ).toHaveLength(1);
    expect(await screen.findByText("Deleted production")).toBeInTheDocument();
    // The deleted project is not fetched again after the DELETE.
    const deleteAt = calls.findIndex((c) => c.method === "DELETE");
    expect(
      calls
        .slice(deleteAt + 1)
        .filter((c) => c.method === "GET" && c.path === `/api/v1/projects/${ID}`),
    ).toHaveLength(0);
  });
});
