// Test helpers: render a page inside all providers with a mocked `fetch`.
import { QueryClientProvider } from "@tanstack/react-query";
import { render } from "@testing-library/react";
import type { ReactElement } from "react";
import { MemoryRouter } from "react-router";
import { vi } from "vitest";
import { makeQueryClient } from "../App";
import { SessionProvider } from "../api/session";
import { ToastProvider } from "../ui/toast";

export type Call = { method: string; path: string; body: unknown; headers: Headers };
type Handler = (call: Call) => { status?: number; body?: unknown } | undefined;

export const PROFILE = {
  user: { id: "u1", email: "owner@example.com", name: "Owner" },
  org: { id: "o1", name: "Example org" },
  role: "owner",
  auth: "session",
  csrf_token: "csrf-1",
};

/** Replaces `fetch` with `handler`; unhandled requests answer 404. Returns the recorded calls. */
export function mockFetch(handler: Handler): Call[] {
  const calls: Call[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn(async (input: Request) => {
      const url = new URL(input.url);
      const text = await input.text();
      const call: Call = {
        method: input.method,
        path: url.pathname + url.search,
        body: text ? JSON.parse(text) : undefined,
        headers: input.headers,
      };
      calls.push(call);
      const res = handler(call) ?? {
        status: 404,
        body: { error: { code: "not_found", message: "not found" } },
      };
      const status = res.status ?? 200;
      return new Response(status === 204 ? null : JSON.stringify(res.body ?? {}), {
        status,
        headers: { "Content-Type": "application/json" },
      });
    }),
  );
  return calls;
}

/** Renders `ui` (normally `<AppRoutes />`) at `path` with all providers. */
export function renderApp(ui: ReactElement, path = "/") {
  return render(
    <QueryClientProvider client={makeQueryClient()}>
      <ToastProvider>
        <SessionProvider>
          <MemoryRouter initialEntries={[path]}>{ui}</MemoryRouter>
        </SessionProvider>
      </ToastProvider>
    </QueryClientProvider>,
  );
}
