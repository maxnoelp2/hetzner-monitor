import { QueryClientProvider } from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { MemoryRouter } from "react-router";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppRoutes, makeQueryClient } from "../App";
import { SessionProvider } from "../api/session";
import { PROFILE, mockFetch, renderApp } from "../test/utils";
import type { Call } from "../test/utils";
import { ToastProvider } from "../ui/toast";

const KEY = {
  id: "k1",
  name: "Grafana",
  prefix: "ab12cd34",
  scopes: ["read"],
  created_at: "2026-11-13T10:00:00Z",
  last_used_at: null,
  revoked_at: null,
};
const REVOKED = { ...KEY, id: "k2", name: "Old script", revoked_at: "2026-11-13T11:00:00Z" };
const SECRET = "hm_ef56gh78_this-is-the-new-test-key";

function api(
  extra?: (c: Call) => { status?: number; body?: unknown } | undefined,
  profile: Record<string, unknown> = PROFILE,
) {
  return mockFetch((c) => {
    const answer = extra?.(c);
    if (answer) return answer;
    if (c.path === "/api/v1/me") return { body: profile };
    if (c.method === "GET" && c.path === "/api/v1/api-keys")
      return { body: { api_keys: [KEY, REVOKED] } };
    return undefined;
  });
}

afterEach(() => vi.unstubAllGlobals());

/** Like `renderApp`, but returns the query client so a test can inspect its caches. */
function renderSettings(): QueryClient {
  const client = makeQueryClient();
  render(
    <QueryClientProvider client={client}>
      <ToastProvider>
        <SessionProvider>
          <MemoryRouter initialEntries={["/settings"]}>
            <AppRoutes />
          </MemoryRouter>
        </SessionProvider>
      </ToastProvider>
    </QueryClientProvider>,
  );
  return client;
}

/** Everything the mutation cache still holds (variables and results), serialised. */
function mutationCacheText(client: QueryClient): string {
  return JSON.stringify(
    client
      .getMutationCache()
      .getAll()
      .map((m) => ({ variables: m.state.variables, data: m.state.data })),
  );
}

describe("SettingsPage account", () => {
  it("shows the profile and blocks a password change when the passwords differ", async () => {
    const calls = api();
    renderApp(<AppRoutes />, "/settings");
    expect(await screen.findByText("owner@example.com")).toBeInTheDocument();
    expect(screen.getAllByText("Example org").length).toBeGreaterThan(0);
    expect(screen.getAllByText("Owner").length).toBeGreaterThan(0);

    await userEvent.type(screen.getByLabelText("Current password"), "old-password-123");
    await userEvent.type(screen.getByLabelText("New password"), "new-password-1234");
    await userEvent.type(screen.getByLabelText("Repeat new password"), "new-password-9999");
    await userEvent.click(screen.getByRole("button", { name: "Change password" }));

    expect(screen.getByText("The passwords do not match.")).toBeInTheDocument();
    expect(calls.some((c) => c.path === "/api/v1/me/password")).toBe(false);
  });

  it("shows a wrong current password on its field, then changes it and clears the form", async () => {
    let wrong = true;
    const calls = api((c) => {
      if (c.method !== "PUT" || c.path !== "/api/v1/me/password") return undefined;
      if (wrong) {
        wrong = false;
        return {
          status: 403,
          body: { error: { code: "wrong_password", message: "wrong password" } },
        };
      }
      return { status: 204 };
    });
    const client = renderSettings();
    const current = await screen.findByLabelText("Current password");
    await userEvent.type(current, "not-the-password");
    await userEvent.type(screen.getByLabelText("New password"), "new-password-1234");
    await userEvent.type(screen.getByLabelText("Repeat new password"), "new-password-1234");
    await userEvent.click(screen.getByRole("button", { name: "Change password" }));

    expect(await screen.findByText("The current password is wrong.")).toBeInTheDocument();
    expect(current).toHaveAttribute("aria-invalid", "true");
    await waitFor(() => expect(mutationCacheText(client)).not.toContain("not-the-password"));
    expect(calls.find((c) => c.method === "PUT")!.body).toEqual({
      current_password: "not-the-password",
      new_password: "new-password-1234",
    });

    await userEvent.clear(current);
    await userEvent.type(current, "old-password-123");
    await userEvent.click(screen.getByRole("button", { name: "Change password" }));
    expect(
      await screen.findByText("Password changed. Other sessions were logged out."),
    ).toBeInTheDocument();
    expect(current).toHaveValue("");
    expect(screen.getByLabelText("New password")).toHaveValue("");
    expect(screen.getByLabelText("Repeat new password")).toHaveValue("");
    // Neither password lingers in the query client.
    await waitFor(() => expect(client.getMutationCache().getAll()).toHaveLength(0));
  });
});

describe("SettingsPage API keys", () => {
  it("lists keys with prefix, scopes and revoked state", async () => {
    api();
    renderApp(<AppRoutes />, "/settings");
    const row = (await screen.findByText("Grafana")).closest("tr")!;
    expect(within(row).getByText("hm_ab12cd34_…")).toBeInTheDocument();
    expect(within(row).getByText("read")).toBeInTheDocument();
    expect(within(row).getByText("Never")).toBeInTheDocument();
    const revoked = screen.getByText("Old script").closest("tr")!;
    expect(within(revoked).getByText("Revoked")).toBeInTheDocument();
    expect(within(revoked).queryByRole("button", { name: "Revoke" })).not.toBeInTheDocument();
  });

  it("shows a created key once and drops it when the dialog closes", async () => {
    const writeText = vi.fn(() => Promise.resolve());
    Object.defineProperty(navigator, "clipboard", { value: { writeText }, configurable: true });
    const calls = api((c) =>
      c.method === "POST" && c.path === "/api/v1/api-keys"
        ? {
            status: 201,
            body: {
              api_key: { ...KEY, id: "k3", name: "CI", scopes: ["read", "write"] },
              key: SECRET,
            },
          }
        : undefined,
    );
    const client = renderSettings();
    await screen.findByText("Grafana");

    await userEvent.click(screen.getByRole("button", { name: "Create API key" }));
    const dialog = screen.getByRole("dialog");
    await userEvent.type(within(dialog).getByLabelText("Name"), "CI");
    await userEvent.click(within(dialog).getByRole("checkbox", { name: "write" }));
    await userEvent.click(within(dialog).getByRole("button", { name: "Create key" }));

    expect(await within(dialog).findByText(SECRET)).toBeInTheDocument();
    expect(within(dialog).getByText(/This key is shown only once\./)).toBeInTheDocument();
    // Only the open dialog's state holds the key, not the query client's mutation cache.
    await waitFor(() => expect(mutationCacheText(client)).not.toContain(SECRET));
    expect(calls.find((c) => c.method === "POST")!.body).toEqual({
      name: "CI",
      scopes: ["read", "write"],
    });

    await userEvent.click(within(dialog).getByRole("button", { name: "Copy" }));
    expect(writeText).toHaveBeenCalledWith(SECRET);
    expect(await screen.findByText("Key copied")).toBeInTheDocument();

    await userEvent.click(within(dialog).getByRole("button", { name: "Done" }));
    await waitFor(() => expect(screen.queryByText(SECRET)).not.toBeInTheDocument());
    expect(mutationCacheText(client)).not.toContain(SECRET);

    // Reopening starts a fresh form; the key is gone for good.
    await userEvent.click(screen.getByRole("button", { name: "Create API key" }));
    expect(within(screen.getByRole("dialog")).getByLabelText("Name")).toHaveValue("");
    expect(screen.queryByText(SECRET)).not.toBeInTheDocument();
  });

  it("revokes a key after confirmation", async () => {
    const calls = api((c) =>
      c.method === "DELETE" && c.path === "/api/v1/api-keys/k1" ? { status: 204 } : undefined,
    );
    renderApp(<AppRoutes />, "/settings");
    const row = (await screen.findByText("Grafana")).closest("tr")!;
    await userEvent.click(within(row).getByRole("button", { name: "Revoke" }));
    const dialog = screen.getByRole("dialog");
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
    await userEvent.click(within(dialog).getByRole("button", { name: "Revoke key" }));
    await waitFor(() => expect(calls.some((c) => c.method === "DELETE")).toBe(true));
    expect(await screen.findByText("API key revoked")).toBeInTheDocument();
  });

  it("shows members a note instead of the key list", async () => {
    const calls = api(undefined, { ...PROFILE, role: "member" });
    renderApp(<AppRoutes />, "/settings");
    expect(
      await screen.findByText("Only owners and admins of the org manage API keys."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Create API key" })).not.toBeInTheDocument();
    expect(calls.some((c) => c.path === "/api/v1/api-keys")).toBe(false);
  });
});
