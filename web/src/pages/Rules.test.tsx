import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppRoutes } from "../App";
import { PROFILE, mockFetch, renderApp } from "../test/utils";
import type { Call } from "../test/utils";

const RULE = {
  id: "r1",
  name: "CPU high",
  check_type: "cpu",
  params: { threshold_pct: 90, for_s: 600 },
  severity: "warning",
  enabled: true,
  is_default: true,
  renotify_s: 0,
  created_at: "2026-11-13T10:00:00Z",
  updated_at: "2026-11-13T10:00:00Z",
  project_id: "p1",
  resource_id: null,
  selector: "env=prod",
};

const CUSTOM = {
  ...RULE,
  id: "r2",
  name: "Disk busy",
  check_type: "disk_io",
  params: { metric: "iops_read", threshold: 1000, for_s: 300 },
  is_default: false,
  project_id: null,
  selector: null,
};

function api(extra?: (c: Call) => { status?: number; body?: unknown } | undefined) {
  return mockFetch((c) => {
    const answer = extra?.(c);
    if (answer) return answer;
    if (c.path === "/api/v1/me") return { body: PROFILE };
    if (c.method === "GET" && (c.path === "/api/v1/rules" || c.path.startsWith("/api/v1/rules?")))
      return { body: { rules: [RULE, CUSTOM] } };
    if (c.method === "GET" && c.path === "/api/v1/projects")
      return { body: { projects: [{ id: "p1", name: "Production" }] } };
    if (c.method === "GET" && c.path === "/api/v1/channels")
      return {
        body: {
          channels: [
            {
              id: "c1",
              name: "Ops mail",
              type: "email",
              hint: "1 recipient",
              min_severity: "warning",
              enabled: true,
            },
          ],
        },
      };
    if (c.method === "GET" && c.path === "/api/v1/rules/r1")
      return { body: { rule: RULE, channel_ids: [] } };
    return undefined;
  });
}

afterEach(() => vi.unstubAllGlobals());

describe("RulesPage", () => {
  it("lists rules with project name, selector and default badge", async () => {
    api();
    renderApp(<AppRoutes />, "/rules");
    expect(await screen.findByText("CPU high")).toBeInTheDocument();
    const row = screen.getByText("CPU high").closest("tr")!;
    expect(within(row).getByText("default")).toBeInTheDocument();
    expect(within(row).getByText("env=prod")).toBeInTheDocument();
    expect(await within(row).findByText("Production")).toBeInTheDocument();
    const custom = screen.getByText("Disk busy").closest("tr")!;
    expect(within(custom).getByText("All projects")).toBeInTheDocument();
    expect(within(custom).getByRole("button", { name: "Delete Disk busy" })).toBeInTheDocument();
    expect(within(row).queryByRole("button", { name: /Delete/ })).not.toBeInTheDocument();
  });

  it("says when project names could not be loaded instead of showing Unknown project", async () => {
    api((c) =>
      c.method === "GET" && c.path === "/api/v1/projects"
        ? { status: 403, body: { error: { code: "forbidden", message: "Not allowed" } } }
        : undefined,
    );
    renderApp(<AppRoutes />, "/rules");
    expect(await screen.findByText(/Could not load the project names/)).toBeInTheDocument();
    const row = screen.getByText("CPU high").closest("tr")!;
    expect(within(row).getByText("Project p1")).toBeInTheDocument();
    expect(screen.queryByText("Unknown project")).not.toBeInTheDocument();
  });

  it("toggling enabled sends PATCH {enabled: false} with the CSRF header", async () => {
    const calls = api((c) =>
      c.method === "PATCH"
        ? { body: { rule: { ...RULE, enabled: false }, channel_ids: [] } }
        : undefined,
    );
    const user = userEvent.setup();
    renderApp(<AppRoutes />, "/rules");
    await user.click(await screen.findByRole("switch", { name: "Enable CPU high" }));
    await waitFor(() => expect(calls.some((c) => c.method === "PATCH")).toBe(true));
    const patch = calls.find((c) => c.method === "PATCH")!;
    expect(patch.path).toBe("/api/v1/rules/r1");
    expect(patch.body).toEqual({ enabled: false });
    expect(patch.headers.get("X-CSRF-Token")).toBe("csrf-1");
  });

  it("creates a rule with the chosen check type and parsed params", async () => {
    const calls = api((c) =>
      c.method === "POST" ? { status: 201, body: { rule: { ...CUSTOM, id: "r3" } } } : undefined,
    );
    const user = userEvent.setup();
    renderApp(<AppRoutes />, "/rules");
    await screen.findByText("CPU high");
    await screen.findAllByText("Production");
    await user.click(screen.getByRole("button", { name: "New rule" }));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("Name"), "Upload busy");
    await user.selectOptions(within(dialog).getByLabelText("Check"), "network");
    const params = within(dialog).getByLabelText("Params") as HTMLTextAreaElement;
    expect(JSON.parse(params.value)).toEqual({
      metric: "bandwidth_out",
      threshold: 10000000,
      for_s: 300,
    });
    await user.clear(params);
    await user.click(params);
    await user.paste('{"metric": "pps_out", "threshold": 5000, "for_s": 600}');
    await user.selectOptions(within(dialog).getByLabelText("Project"), "p1");
    await user.click(within(dialog).getByRole("button", { name: "Create rule" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    const post = calls.find((c) => c.method === "POST")!;
    expect(post.path).toBe("/api/v1/rules");
    expect(post.body).toEqual({
      name: "Upload busy",
      check_type: "network",
      severity: "warning",
      params: { metric: "pps_out", threshold: 5000, for_s: 600 },
      enabled: true,
      project_id: "p1",
    });
    expect(post.headers.get("X-CSRF-Token")).toBe("csrf-1");
  });

  it("rejects invalid params JSON without a request", async () => {
    const calls = api();
    const user = userEvent.setup();
    renderApp(<AppRoutes />, "/rules");
    await user.click(await screen.findByRole("button", { name: "New rule" }));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("Name"), "Broken");
    const params = within(dialog).getByLabelText("Params");
    await user.clear(params);
    await user.click(params);
    await user.paste("{not json");
    await user.click(within(dialog).getByRole("button", { name: "Create rule" }));
    expect(await within(dialog).findByText(/not valid JSON/)).toBeInTheDocument();
    expect(calls.some((c) => c.method !== "GET")).toBe(false);
  });

  it("edits a rule: clears the selector, pins a channel and shows server errors", async () => {
    const calls = api((c) =>
      c.method === "PATCH"
        ? {
            status: 400,
            body: {
              error: { code: "bad_request", message: "params: threshold_pct must be 1-100" },
            },
          }
        : undefined,
    );
    const user = userEvent.setup();
    renderApp(<AppRoutes />, "/rules");
    await user.click(await screen.findByRole("button", { name: "Edit CPU high" }));
    const dialog = await screen.findByRole("dialog");
    await user.clear(await within(dialog).findByLabelText("Selector"));
    await user.click(within(dialog).getByLabelText("Ops mail"));
    await user.click(within(dialog).getByRole("button", { name: "Save rule" }));
    expect(
      await within(dialog).findByText("params: threshold_pct must be 1-100"),
    ).toBeInTheDocument();
    const patch = calls.find((c) => c.method === "PATCH")!;
    expect(patch.path).toBe("/api/v1/rules/r1");
    expect(patch.body).toEqual({
      name: "CPU high",
      severity: "warning",
      selector: null,
      params: { threshold_pct: 90, for_s: 600 },
      channel_ids: ["c1"],
    });
  });

  it("deletes a custom rule after confirmation", async () => {
    const calls = api((c) => (c.method === "DELETE" ? { status: 204 } : undefined));
    const user = userEvent.setup();
    renderApp(<AppRoutes />, "/rules");
    await user.click(await screen.findByRole("button", { name: "Delete Disk busy" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Delete rule" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE")).toBe(true));
    expect(calls.find((c) => c.method === "DELETE")!.path).toBe("/api/v1/rules/r2");
  });

  it("keeps the project filter in the URL and sends it to the API", async () => {
    const calls = api();
    renderApp(<AppRoutes />, "/rules?project_id=p1");
    await screen.findByText("CPU high");
    expect(calls.some((c) => c.path === "/api/v1/rules?project_id=p1")).toBe(true);
  });
});
