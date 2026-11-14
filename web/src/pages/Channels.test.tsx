import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppRoutes } from "../App";
import { PROFILE, mockFetch, renderApp } from "../test/utils";
import type { Call } from "../test/utils";

const HOOK = {
  id: "c1",
  name: "Ops hook",
  type: "webhook",
  hint: "hooks.example.com",
  min_severity: "warning",
  enabled: true,
  created_at: "2026-11-13T10:00:00Z",
  updated_at: "2026-11-13T10:00:00Z",
};

function api(extra?: (c: Call) => { status?: number; body?: unknown } | undefined) {
  return mockFetch((c) => {
    const answer = extra?.(c);
    if (answer) return answer;
    if (c.path === "/api/v1/me") return { body: PROFILE };
    if (c.method === "GET" && c.path === "/api/v1/channels") return { body: { channels: [HOOK] } };
    return undefined;
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("ChannelsPage", () => {
  it("lists channels with type, hint and severity", async () => {
    api();
    renderApp(<AppRoutes />, "/channels");
    const row = (await screen.findByText("Ops hook")).closest("tr")!;
    expect(within(row).getByText("Webhook")).toBeInTheDocument();
    expect(within(row).getByText("hooks.example.com")).toBeInTheDocument();
    expect(within(row).getByText("Warning")).toBeInTheDocument();
    expect(within(row).getByRole("switch")).toBeChecked();
  });

  it("shows the empty state", async () => {
    api((c) =>
      c.method === "GET" && c.path === "/api/v1/channels" ? { body: { channels: [] } } : undefined,
    );
    renderApp(<AppRoutes />, "/channels");
    expect(await screen.findByText("No channels yet")).toBeInTheDocument();
  });

  it("creates a webhook channel with a generated secret", async () => {
    const user = userEvent.setup();
    const calls = api((c) =>
      c.method === "POST" && c.path === "/api/v1/channels"
        ? { status: 201, body: { channel: { ...HOOK, id: "c2", name: "Receiver" } } }
        : undefined,
    );
    renderApp(<AppRoutes />, "/channels");
    await screen.findByText("Ops hook");

    await user.click(screen.getByRole("button", { name: "Add channel" }));
    const dialog = await screen.findByRole("dialog");
    await user.selectOptions(within(dialog).getByLabelText("Type"), "webhook");
    await user.type(within(dialog).getByLabelText("Name"), "Receiver");
    await user.type(within(dialog).getByLabelText("URL"), "https://hooks.example.com/hm");
    await user.click(within(dialog).getByRole("button", { name: "Generate" }));
    const secret = (within(dialog).getByLabelText("Signing secret") as HTMLInputElement).value;
    expect(secret).toMatch(/^[A-Za-z0-9_-]{43}$/);
    await user.selectOptions(within(dialog).getByLabelText("Minimum severity"), "critical");
    await user.click(within(dialog).getByRole("button", { name: "Add channel" }));

    await screen.findByText("Channel added");
    const post = calls.find((c) => c.method === "POST" && c.path === "/api/v1/channels");
    expect(post?.body).toEqual({
      type: "webhook",
      name: "Receiver",
      config: { url: "https://hooks.example.com/hm", secret },
      min_severity: "critical",
      enabled: true,
    });
    expect(post?.headers.get("X-CSRF-Token")).toBe("csrf-1");
  });

  it("splits email recipients and shows name_taken on the name field", async () => {
    const user = userEvent.setup();
    const calls = api((c) =>
      c.method === "POST" && c.path === "/api/v1/channels"
        ? {
            status: 409,
            body: { error: { code: "name_taken", message: "a channel with this name exists" } },
          }
        : undefined,
    );
    renderApp(<AppRoutes />, "/channels");
    await screen.findByText("Ops hook");

    await user.click(screen.getByRole("button", { name: "Add channel" }));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("Name"), "Ops hook");
    await user.type(within(dialog).getByLabelText("Recipients"), "a@example.com, b@example.com");
    await user.click(within(dialog).getByRole("button", { name: "Add channel" }));

    expect(
      await within(dialog).findByText("Another channel already has this name."),
    ).toBeInTheDocument();
    const post = calls.find((c) => c.method === "POST" && c.path === "/api/v1/channels");
    expect(post?.body).toMatchObject({
      type: "email",
      config: { to: ["a@example.com", "b@example.com"] },
      min_severity: "warning",
    });
  });

  it("explains email_disabled", async () => {
    const user = userEvent.setup();
    api((c) =>
      c.method === "POST" && c.path === "/api/v1/channels"
        ? { status: 422, body: { error: { code: "email_disabled", message: "no smtp" } } }
        : undefined,
    );
    renderApp(<AppRoutes />, "/channels");
    await screen.findByText("Ops hook");

    await user.click(screen.getByRole("button", { name: "Add channel" }));
    const dialog = await screen.findByRole("dialog");
    await user.type(within(dialog).getByLabelText("Name"), "Mail");
    await user.type(within(dialog).getByLabelText("Recipients"), "a@example.com");
    await user.click(within(dialog).getByRole("button", { name: "Add channel" }));

    expect(await within(dialog).findByText(/HM_SMTP_HOST/)).toBeInTheDocument();
  });

  it("creates a Web Push channel without config and reports tests per device", async () => {
    const user = userEvent.setup();
    const PUSH = { ...HOOK, id: "c3", name: "Phones", type: "web_push", hint: "all members" };
    const calls = api((c) => {
      if (c.method === "POST" && c.path === "/api/v1/channels")
        return { status: 201, body: { channel: PUSH } };
      if (c.method === "GET" && c.path === "/api/v1/channels")
        return { body: { channels: [PUSH] } };
      if (c.path === "/api/v1/channels/c3/test")
        return {
          body: {
            ok: false,
            error: "1 of 2 devices failed",
            devices: [
              { id: "s1", name: "Pixel", ok: true },
              { id: "s2", name: "Old iPad", ok: false, error: "device unsubscribed (HTTP 410)" },
            ],
          },
        };
      return undefined;
    });
    renderApp(<AppRoutes />, "/channels");
    await screen.findByText("Phones");

    await user.click(screen.getByRole("button", { name: "Add channel" }));
    const dialog = await screen.findByRole("dialog");
    await user.selectOptions(within(dialog).getByLabelText("Type"), "web_push");
    expect(within(dialog).getByText(/every device a member of the org/)).toBeInTheDocument();
    await user.type(within(dialog).getByLabelText("Name"), "Phones");
    await user.click(within(dialog).getByRole("button", { name: "Add channel" }));
    await screen.findByText("Channel added");
    const post = calls.find((c) => c.method === "POST" && c.path === "/api/v1/channels");
    expect(post?.body).toMatchObject({ type: "web_push", name: "Phones", config: {} });

    const row = screen
      .getAllByText("Phones")
      .map((el) => el.closest("tr"))
      .find(Boolean)!;
    expect(within(row).getByText("Web Push")).toBeInTheDocument();
    await user.click(within(row).getByRole("button", { name: /test/i }));
    expect(
      await screen.findByText("Pixel: sent; Old iPad: device unsubscribed (HTTP 410)"),
    ).toBeInTheDocument();
  });

  it("explains push_disabled", async () => {
    const user = userEvent.setup();
    api((c) =>
      c.method === "POST" && c.path === "/api/v1/channels"
        ? { status: 422, body: { error: { code: "push_disabled", message: "no vapid" } } }
        : undefined,
    );
    renderApp(<AppRoutes />, "/channels");
    await screen.findByText("Ops hook");
    await user.click(screen.getByRole("button", { name: "Add channel" }));
    const dialog = await screen.findByRole("dialog");
    await user.selectOptions(within(dialog).getByLabelText("Type"), "web_push");
    await user.type(within(dialog).getByLabelText("Name"), "Phones");
    await user.click(within(dialog).getByRole("button", { name: "Add channel" }));
    expect(await within(dialog).findByText(/HM_VAPID_SUBJECT/)).toBeInTheDocument();
  });

  it("shows the error toast when a test fails", async () => {
    const user = userEvent.setup();
    const calls = api((c) =>
      c.method === "POST" && c.path === "/api/v1/channels/c1/test"
        ? { body: { ok: false, error: "HTTP 404", retryable: false } }
        : undefined,
    );
    renderApp(<AppRoutes />, "/channels");
    await screen.findByText("Ops hook");

    await user.click(screen.getByRole("button", { name: "Send test" }));

    expect(await screen.findByText("Test failed")).toBeInTheDocument();
    expect(screen.getByText("HTTP 404")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST" && c.path === "/api/v1/channels/c1/test")).toBe(
      true,
    );
  });

  it("marks a retryable test failure as temporary", async () => {
    const user = userEvent.setup();
    api((c) =>
      c.method === "POST" && c.path === "/api/v1/channels/c1/test"
        ? { body: { ok: false, error: "timeout", retryable: true } }
        : undefined,
    );
    renderApp(<AppRoutes />, "/channels");
    await screen.findByText("Ops hook");

    await user.click(screen.getByRole("button", { name: "Send test" }));
    expect(await screen.findByText("timeout (temporary, try again)")).toBeInTheDocument();
  });

  it("toggles enabled with a PATCH", async () => {
    const user = userEvent.setup();
    const calls = api((c) =>
      c.method === "PATCH" && c.path === "/api/v1/channels/c1"
        ? { body: { channel: { ...HOOK, enabled: false } } }
        : undefined,
    );
    renderApp(<AppRoutes />, "/channels");
    await screen.findByText("Ops hook");

    await user.click(screen.getByRole("switch", { name: "Ops hook enabled" }));
    await waitFor(() =>
      expect(calls.find((c) => c.method === "PATCH")?.body).toEqual({ enabled: false }),
    );
  });

  it("edits without replacing the config", async () => {
    const user = userEvent.setup();
    const calls = api((c) =>
      c.method === "PATCH" && c.path === "/api/v1/channels/c1"
        ? { body: { channel: { ...HOOK, name: "Ops receiver" } } }
        : undefined,
    );
    renderApp(<AppRoutes />, "/channels");
    await screen.findByText("Ops hook");

    await user.click(screen.getByRole("button", { name: "Edit Ops hook" }));
    const dialog = await screen.findByRole("dialog");
    expect(within(dialog).getByText("hooks.example.com")).toBeInTheDocument();
    expect(within(dialog).queryByLabelText("Signing secret")).not.toBeInTheDocument();
    const name = within(dialog).getByLabelText("Name");
    await user.clear(name);
    await user.type(name, "Ops receiver");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));

    await screen.findByText("Channel saved");
    const patch = calls.find((c) => c.method === "PATCH");
    expect(patch?.body).toEqual({ name: "Ops receiver", min_severity: "warning", enabled: true });
  });

  it("replaces the config when asked", async () => {
    const user = userEvent.setup();
    const calls = api((c) =>
      c.method === "PATCH" && c.path === "/api/v1/channels/c1"
        ? { body: { channel: HOOK } }
        : undefined,
    );
    renderApp(<AppRoutes />, "/channels");
    await screen.findByText("Ops hook");

    await user.click(screen.getByRole("button", { name: "Edit Ops hook" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("switch", { name: "Replace configuration" }));
    await user.type(within(dialog).getByLabelText("URL"), "https://new.example.com/hm");
    await user.type(within(dialog).getByLabelText("Signing secret"), "0123456789abcdef");
    await user.click(within(dialog).getByRole("button", { name: "Save" }));

    await screen.findByText("Channel saved");
    expect(calls.find((c) => c.method === "PATCH")?.body).toMatchObject({
      config: { url: "https://new.example.com/hm", secret: "0123456789abcdef" },
    });
  });

  it("deletes after confirmation", async () => {
    const user = userEvent.setup();
    const calls = api((c) =>
      c.method === "DELETE" && c.path === "/api/v1/channels/c1" ? { status: 204 } : undefined,
    );
    renderApp(<AppRoutes />, "/channels");
    await screen.findByText("Ops hook");

    await user.click(screen.getByRole("button", { name: "Delete Ops hook" }));
    const dialog = await screen.findByRole("dialog");
    expect(calls.some((c) => c.method === "DELETE")).toBe(false);
    await user.click(within(dialog).getByRole("button", { name: "Delete channel" }));

    await screen.findByText("Channel deleted");
    expect(calls.some((c) => c.method === "DELETE" && c.path === "/api/v1/channels/c1")).toBe(true);
  });
});
