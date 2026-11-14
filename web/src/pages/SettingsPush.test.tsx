// Settings → Push notifications. The browser's push APIs are mocked (`../push`); the server is
// mocked with `mockFetch`.
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppRoutes } from "../App";
import { PROFILE, mockFetch, renderApp } from "../test/utils";
import type { Call } from "../test/utils";

const push = vi.hoisted(() => ({
  support: "supported" as string,
  permission: "default" as NotificationPermission,
  device: null as string | null,
  subscribe: vi.fn(),
  unsubscribe: vi.fn(),
}));

vi.mock("../push", async (original) => {
  const real = await original<typeof import("../push")>();
  return {
    ...real,
    pushSupport: () => push.support,
    permission: () => push.permission,
    subscribe: push.subscribe,
    unsubscribe: push.unsubscribe,
    deviceName: () => "Pixel · Chrome",
    thisDevice: () => push.device,
    rememberDevice: (id: string | null) => {
      push.device = id;
    },
  };
});

const KEY = "BOrq-public-vapid-key";
const PHONE = {
  id: "s1",
  name: "Pixel · Chrome",
  host: "fcm.googleapis.com",
  created_at: "2026-11-14T10:00:00Z",
  updated_at: "2026-11-14T10:00:00Z",
};
const LAPTOP = { ...PHONE, id: "s2", name: "Mac · Safari", host: "web.push.apple.com" };
const PUSH_CHANNEL = {
  id: "c1",
  name: "Phones",
  type: "web_push",
  hint: "all members",
  min_severity: "warning",
  enabled: true,
  created_at: "2026-11-13T10:00:00Z",
  updated_at: "2026-11-13T10:00:00Z",
};

function api(
  devices: unknown[],
  extra?: (c: Call) => { status?: number; body?: unknown } | undefined,
  options: { enabled?: boolean; channels?: unknown[] } = {},
) {
  const enabled = options.enabled ?? true;
  return mockFetch((c) => {
    const answer = extra?.(c);
    if (answer) return answer;
    if (c.path === "/api/v1/me") return { body: PROFILE };
    if (c.path === "/api/v1/push/config")
      return { body: enabled ? { enabled, vapid_public_key: KEY } : { enabled } };
    if (c.method === "GET" && c.path === "/api/v1/push/subscriptions")
      return { body: { subscriptions: devices } };
    if (c.method === "GET" && c.path === "/api/v1/channels")
      return { body: { channels: options.channels ?? [PUSH_CHANNEL] } };
    if (c.path === "/api/v1/api-keys") return { body: { api_keys: [] } };
    return undefined;
  });
}

async function pushCard() {
  const title = await screen.findByText("Push notifications");
  return title.closest(".card") as HTMLElement;
}

beforeEach(() => {
  push.support = "supported";
  push.permission = "default";
  push.device = null;
  push.subscribe.mockReset();
  push.unsubscribe.mockReset().mockResolvedValue(undefined);
});

afterEach(() => vi.unstubAllGlobals());

describe("Settings push notifications", () => {
  it("explains a server without Web Push", async () => {
    api([], undefined, { enabled: false });
    renderApp(<AppRoutes />, "/settings");
    const card = await pushCard();
    expect(await within(card).findByText(/Web Push is not set up on this server/)).toBeVisible();
    expect(within(card).queryByRole("button")).not.toBeInTheDocument();
  });

  it("turns push on for this device and marks it", async () => {
    const user = userEvent.setup();
    let devices: unknown[] = [];
    push.subscribe.mockImplementation(async () => {
      push.permission = "granted";
      return {
        endpoint: "https://fcm.googleapis.com/fcm/send/x",
        keys: { p256dh: "p", auth: "a" },
      };
    });
    const calls = api(devices, (c) => {
      if (c.method === "POST" && c.path === "/api/v1/push/subscriptions") {
        devices = [PHONE];
        return { status: 201, body: { subscription: PHONE } };
      }
      if (c.method === "GET" && c.path === "/api/v1/push/subscriptions")
        return { body: { subscriptions: devices } };
      return undefined;
    });
    renderApp(<AppRoutes />, "/settings");
    const card = await pushCard();
    expect(await within(card).findByText("No devices")).toBeInTheDocument();

    await user.click(within(card).getByRole("button", { name: "Turn on push for this device" }));
    expect(await screen.findByText("Push turned on for this device")).toBeInTheDocument();
    expect(push.subscribe).toHaveBeenCalledWith(KEY);
    const post = calls.find((c) => c.method === "POST" && c.path === "/api/v1/push/subscriptions");
    expect(post?.body).toEqual({
      endpoint: "https://fcm.googleapis.com/fcm/send/x",
      keys: { p256dh: "p", auth: "a" },
      name: "Pixel · Chrome",
    });
    expect(post?.headers.get("X-CSRF-Token")).toBe("csrf-1");
    expect(push.device).toBe("s1");
    const row = await waitFor(() => {
      const badge = within(card)
        .getAllByText("This device")
        .find((el) => el.closest("tr"));
      if (!badge) throw new Error("no device badge yet");
      return badge.closest("tr")!;
    });
    expect(within(row).getByText(/Pixel · Chrome/)).toBeInTheDocument();
    expect(within(card).getByText("On")).toBeInTheDocument();
  });

  it("tells the user when notifications are blocked", async () => {
    const user = userEvent.setup();
    const { PushDenied } = await import("../push");
    push.subscribe.mockImplementation(async () => {
      push.permission = "denied";
      throw new PushDenied();
    });
    const calls = api([]);
    renderApp(<AppRoutes />, "/settings");
    const card = await pushCard();
    await user.click(
      await within(card).findByRole("button", { name: "Turn on push for this device" }),
    );
    expect(await screen.findByText(/Notifications are blocked\. Allow them/)).toBeInTheDocument();
    expect(calls.some((c) => c.method === "POST")).toBe(false);
    await waitFor(() =>
      expect(within(card).getByRole("button", { name: /Turn on push/ })).toBeDisabled(),
    );
  });

  it.each([
    ["insecure", /Push needs HTTPS/],
    ["ios-not-installed", /Add to Home Screen/],
    ["unsupported", /does not support push/],
  ])("explains why push is unavailable (%s)", async (support, text) => {
    push.support = support;
    api([LAPTOP]);
    renderApp(<AppRoutes />, "/settings");
    const card = await pushCard();
    expect(await within(card).findByText(text)).toBeInTheDocument();
    expect(within(card).queryByRole("button", { name: /Turn on push/ })).not.toBeInTheDocument();
    // Other devices can still be managed from here.
    expect(await within(card).findByText("Mac · Safari")).toBeInTheDocument();
  });

  it("sends a test push and reports a gone device", async () => {
    const user = userEvent.setup();
    push.device = "s1";
    const calls = api([PHONE, LAPTOP], (c) => {
      if (c.path === "/api/v1/push/subscriptions/s1/test") return { body: { ok: true } };
      if (c.path === "/api/v1/push/subscriptions/s2/test")
        return { body: { ok: false, error: "device unsubscribed (HTTP 410)", gone: true } };
      return undefined;
    });
    renderApp(<AppRoutes />, "/settings");
    const card = await pushCard();
    await user.click(await within(card).findByRole("button", { name: "Send test push" }));
    expect(await screen.findByText("Test push sent")).toBeInTheDocument();

    const laptop = within(card).getByText("Mac · Safari").closest("tr")!;
    await user.click(within(laptop).getByRole("button", { name: "Send test" }));
    expect(await screen.findByText(/no longer knows Mac · Safari/)).toBeInTheDocument();
    expect(calls.filter((c) => c.path.endsWith("/test")).map((c) => c.method)).toEqual([
      "POST",
      "POST",
    ]);
  });

  it("turns push off for this device", async () => {
    const user = userEvent.setup();
    push.device = "s1";
    const calls = api([PHONE], (c) =>
      c.method === "DELETE" && c.path === "/api/v1/push/subscriptions/s1"
        ? { status: 204 }
        : undefined,
    );
    renderApp(<AppRoutes />, "/settings");
    const card = await pushCard();
    await user.click(await within(card).findByRole("button", { name: "Turn off" }));
    expect(await screen.findByText("Push turned off for this device")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "DELETE")).toBe(true);
    expect(push.unsubscribe).toHaveBeenCalled();
    expect(push.device).toBeNull();
  });

  it("removes another device after confirming and points to a missing channel", async () => {
    const user = userEvent.setup();
    const calls = api(
      [LAPTOP],
      (c) =>
        c.method === "DELETE" && c.path === "/api/v1/push/subscriptions/s2"
          ? { status: 204 }
          : undefined,
      { channels: [] },
    );
    renderApp(<AppRoutes />, "/settings");
    const card = await pushCard();
    expect(await within(card).findByText(/Add one on the Channels page/)).toBeInTheDocument();
    const row = (await within(card).findByText("Mac · Safari")).closest("tr")!;
    await user.click(within(row).getByRole("button", { name: "Remove" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Remove device" }));
    expect(await screen.findByText("Device removed")).toBeInTheDocument();
    expect(calls.some((c) => c.method === "DELETE")).toBe(true);
    expect(push.unsubscribe).not.toHaveBeenCalled();
  });
});
