import { fireEvent, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppRoutes } from "../App";
import { PROFILE, mockFetch, renderApp } from "../test/utils";
import type { Call } from "../test/utils";

const HOUR = 3600_000;
const DAY = 24 * HOUR;
const iso = (offset: number) => new Date(Date.now() + offset).toISOString();

const PROJECT = { id: "p1", name: "Production" };
const RULE = { id: "r1", name: "CPU high", project_id: "p1" };

function silence(overrides: Record<string, unknown> = {}) {
  return {
    id: "s1",
    starts_at: iso(-HOUR),
    ends_at: iso(HOUR),
    reason: "Kernel update",
    created_at: iso(-HOUR),
    created_by: "u1",
    project_id: null,
    resource_id: null,
    rule_id: null,
    selector: null,
    ...overrides,
  };
}

const ACTIVE = silence({ id: "s1", project_id: "p1", rule_id: "r1" });
const SCHEDULED = silence({
  id: "s2",
  starts_at: iso(DAY),
  ends_at: iso(2 * DAY),
  selector: "env=prod",
  reason: "",
});
const ENDED = silence({ id: "s3", starts_at: iso(-2 * DAY), ends_at: iso(-DAY), reason: "Old" });

function api(extra?: (c: Call) => { status?: number; body?: unknown } | undefined) {
  return mockFetch((c) => {
    const answer = extra?.(c);
    if (answer) return answer;
    if (c.path === "/api/v1/me") return { body: PROFILE };
    if (c.method === "GET" && c.path === "/api/v1/silences?active=true")
      return { body: { silences: [ACTIVE, SCHEDULED] } };
    if (c.method === "GET" && c.path === "/api/v1/silences?active=false")
      return { body: { silences: [ACTIVE, SCHEDULED, ENDED] } };
    if (c.method === "GET" && c.path === "/api/v1/projects")
      return { body: { projects: [PROJECT] } };
    if (c.method === "GET" && c.path.startsWith("/api/v1/rules"))
      return { body: { rules: [RULE] } };
    return undefined;
  });
}

/** `yyyy-MM-ddTHH:mm` in local time, the value format of `<input type="datetime-local">`. */
function localInput(d: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

afterEach(() => vi.unstubAllGlobals());

describe("SilencesPage", () => {
  it("lists active silences with scope and status, and shows ended ones on request", async () => {
    const calls = api();
    renderApp(<AppRoutes />, "/silences");

    const active = (await screen.findByText("Kernel update")).closest("tr")!;
    expect(await within(active).findByText(/Production/)).toBeInTheDocument();
    expect(await within(active).findByText(/CPU high/)).toBeInTheDocument();
    expect(within(active).getByText("Active")).toBeInTheDocument();
    expect(within(active).getByRole("button", { name: "End now" })).toBeInTheDocument();

    const scheduled = screen.getByText("env=prod").closest("tr")!;
    expect(within(scheduled).getByText("Scheduled")).toBeInTheDocument();
    expect(screen.queryByText("Old")).not.toBeInTheDocument();

    await userEvent.click(screen.getByRole("switch", { name: "Show ended" }));
    const ended = (await screen.findByText("Old")).closest("tr")!;
    expect(within(ended).getByText("Ended")).toBeInTheDocument();
    expect(within(ended).getByText("Whole org")).toBeInTheDocument();
    expect(within(ended).queryByRole("button")).not.toBeInTheDocument();
    expect(calls.some((c) => c.path === "/api/v1/silences?active=false")).toBe(true);
  });

  it("creates a silence with an ISO UTC end from a preset", async () => {
    const calls = api((c) =>
      c.method === "POST" && c.path === "/api/v1/silences"
        ? { status: 201, body: { silence: ACTIVE } }
        : undefined,
    );
    renderApp(<AppRoutes />, "/silences");
    await screen.findByText("Kernel update");

    await userEvent.click(screen.getByRole("button", { name: "New silence" }));
    const dialog = screen.getByRole("dialog");
    await within(dialog).findByRole("option", { name: "Production" });
    await userEvent.selectOptions(within(dialog).getByLabelText("Project"), "p1");
    await waitFor(() =>
      expect(calls.some((c) => c.path === "/api/v1/rules?project_id=p1")).toBe(true),
    );
    await userEvent.selectOptions(within(dialog).getByLabelText("Duration"), "4h");
    await userEvent.type(within(dialog).getByLabelText("Reason"), "Maintenance");
    const before = Date.now();
    await userEvent.click(within(dialog).getByRole("button", { name: "Create silence" }));

    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    const post = calls.find((c) => c.method === "POST")!;
    const body = post.body as { ends_at: string } & Record<string, unknown>;
    expect(body.ends_at).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\.\d{3}Z$/);
    const end = Date.parse(body.ends_at);
    expect(end).toBeGreaterThanOrEqual(before + 4 * HOUR - 60_000);
    expect(end).toBeLessThanOrEqual(Date.now() + 4 * HOUR);
    expect(body).not.toHaveProperty("starts_at");
    expect(body.project_id).toBe("p1");
    expect(body.reason).toBe("Maintenance");
    expect(post.headers.get("X-CSRF-Token")).toBe("csrf-1");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("converts a custom local end and rejects one more than 365 days ahead", async () => {
    const calls = api((c) =>
      c.method === "POST" && c.path === "/api/v1/silences"
        ? { status: 201, body: { silence: ACTIVE } }
        : undefined,
    );
    renderApp(<AppRoutes />, "/silences");
    await screen.findByText("Kernel update");

    await userEvent.click(screen.getByRole("button", { name: "New silence" }));
    const dialog = screen.getByRole("dialog");
    await userEvent.selectOptions(within(dialog).getByLabelText("Duration"), "custom");
    const endInput = within(dialog).getByLabelText("Ends");

    fireEvent.change(endInput, { target: { value: localInput(new Date(Date.now() + 400 * DAY)) } });
    await userEvent.click(within(dialog).getByRole("button", { name: "Create silence" }));
    expect(within(dialog).getByText("A silence lasts at most 365 days.")).toBeInTheDocument();

    fireEvent.change(endInput, { target: { value: localInput(new Date(Date.now() - DAY)) } });
    await userEvent.click(within(dialog).getByRole("button", { name: "Create silence" }));
    expect(within(dialog).getByText("The end must be in the future.")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST")).toBe(false);

    const local = localInput(new Date(Date.now() + 2 * DAY));
    fireEvent.change(endInput, { target: { value: local } });
    await userEvent.click(within(dialog).getByRole("button", { name: "Create silence" }));
    await waitFor(() => expect(calls.some((c) => c.method === "POST")).toBe(true));
    const body = calls.find((c) => c.method === "POST")!.body as Record<string, string>;
    expect(body.ends_at).toBe(new Date(local).toISOString());
  });

  it("ends a silence after confirmation", async () => {
    const calls = api((c) =>
      c.method === "DELETE" && c.path === "/api/v1/silences/s1" ? { status: 204 } : undefined,
    );
    renderApp(<AppRoutes />, "/silences");
    const row = (await screen.findByText("Kernel update")).closest("tr")!;

    await userEvent.click(within(row).getByRole("button", { name: "End now" }));
    const dialog = screen.getByRole("dialog");
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: "End now" }));

    await waitFor(() =>
      expect(calls.some((c) => c.method === "DELETE" && c.path === "/api/v1/silences/s1")).toBe(
        true,
      ),
    );
    expect(await screen.findByText("Silence ended")).toBeInTheDocument();
  });
});
