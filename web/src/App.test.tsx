import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, it, vi } from "vitest";
import { AppRoutes } from "./App";
import { PROFILE, mockFetch, renderApp } from "./test/utils";
import type { Call } from "./test/utils";

const UNAUTHORIZED = {
  status: 401,
  body: { error: { code: "unauthorized", message: "not logged in" } },
};
const LOGGED_IN = { ...PROFILE, csrf_token: "csrf-from-login" };
const EMPTY_ALERTS = {
  alerts: [],
  meta: {
    pagination: {
      page: 1,
      per_page: 25,
      last_page: 1,
      total_entries: 0,
      next_page: null,
      previous_page: null,
    },
  },
};

/** A server with one account: `/me` answers 401 until the login succeeded. */
function server(extra?: (c: Call) => { status?: number; body?: unknown } | undefined) {
  const state = { session: false };
  const calls = mockFetch((c) => {
    const answer = extra?.(c);
    if (answer) return answer;
    if (c.method === "POST" && c.path === "/api/v1/auth/login") {
      state.session = true;
      return { body: LOGGED_IN };
    }
    if (c.method === "POST" && c.path === "/api/v1/auth/logout") {
      state.session = false;
      return { status: 204 };
    }
    if (!state.session) return UNAUTHORIZED;
    if (c.path === "/api/v1/me") return { body: LOGGED_IN };
    if (c.method === "GET" && c.path.startsWith("/api/v1/alerts")) return { body: EMPTY_ALERTS };
    if (c.method === "GET" && c.path === "/api/v1/projects")
      return { body: { projects: [{ id: "p1", name: "Production" }] } };
    return undefined;
  });
  return { calls, state };
}

async function logIn() {
  await userEvent.type(await screen.findByLabelText("E-mail"), "owner@example.com");
  await userEvent.type(screen.getByLabelText("Password"), "correct-horse-battery");
  await userEvent.click(screen.getByRole("button", { name: "Log in" }));
  expect(await screen.findByRole("heading", { name: "Alerts" })).toBeInTheDocument();
}

afterEach(() => vi.unstubAllGlobals());

describe("App session", () => {
  it("shows the login page to anonymous visitors", async () => {
    server();
    renderApp(<AppRoutes />, "/alerts");
    expect(await screen.findByRole("heading", { name: "Log in" })).toBeInTheDocument();
    expect(screen.queryByRole("navigation", { name: "Main" })).not.toBeInTheDocument();
  });

  it("logs in with e-mail and password and shows the alerts page", async () => {
    const { calls } = server();
    renderApp(<AppRoutes />, "/alerts");
    await logIn();

    const login = calls.find((c) => c.path === "/api/v1/auth/login")!;
    expect(login.method).toBe("POST");
    expect(login.body).toEqual({ email: "owner@example.com", password: "correct-horse-battery" });
    await waitFor(() =>
      expect(calls.some((c) => c.method === "GET" && c.path.startsWith("/api/v1/alerts"))).toBe(
        true,
      ),
    );
  });

  it("shows a failed login without leaving the login page", async () => {
    server((c) =>
      c.path === "/api/v1/auth/login"
        ? {
            status: 401,
            body: { error: { code: "invalid_credentials", message: "invalid credentials" } },
          }
        : undefined,
    );
    renderApp(<AppRoutes />, "/alerts");
    await userEvent.type(await screen.findByLabelText("E-mail"), "owner@example.com");
    await userEvent.type(screen.getByLabelText("Password"), "wrong-password");
    await userEvent.click(screen.getByRole("button", { name: "Log in" }));
    expect(await screen.findByText("E-mail or password is wrong.")).toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Log in" })).toBeInTheDocument();
  });

  it("returns to the login page when a later API call answers 401", async () => {
    const { state } = server();
    renderApp(<AppRoutes />, "/alerts");
    await logIn();

    // The session expires on the server; the next request fails with 401.
    state.session = false;
    await userEvent.click(screen.getByRole("link", { name: "Rules" }));
    expect(await screen.findByRole("heading", { name: "Log in" })).toBeInTheDocument();
  });

  it("sends the CSRF token from the login response with non-GET requests and logs out", async () => {
    const { calls } = server();
    renderApp(<AppRoutes />, "/alerts");
    await logIn();
    expect(
      calls.find((c) => c.path === "/api/v1/auth/login")!.headers.get("X-CSRF-Token"),
    ).toBeNull();

    await userEvent.click(screen.getByRole("button", { name: "Log out" }));
    expect(await screen.findByRole("heading", { name: "Log in" })).toBeInTheDocument();

    const logout = calls.find((c) => c.path === "/api/v1/auth/logout")!;
    expect(logout.method).toBe("POST");
    expect(logout.headers.get("X-CSRF-Token")).toBe("csrf-from-login");
    const gets = calls.filter((c) => c.method === "GET");
    expect(gets.every((c) => c.headers.get("X-CSRF-Token") === null)).toBe(true);
  });

  it("removes this browser's push device before logging out", async () => {
    const { calls } = server();
    localStorage.setItem("hm-push-subscription-id", "dev-1");
    renderApp(<AppRoutes />, "/alerts");
    await logIn();

    await userEvent.click(screen.getByRole("button", { name: "Log out" }));
    expect(await screen.findByRole("heading", { name: "Log in" })).toBeInTheDocument();

    const paths = calls.filter((c) => c.method !== "GET").map((c) => `${c.method} ${c.path}`);
    expect(paths.slice(-2)).toEqual([
      "DELETE /api/v1/push/subscriptions/dev-1",
      "POST /api/v1/auth/logout",
    ]);
    expect(localStorage.getItem("hm-push-subscription-id")).toBeNull();
  });
});
