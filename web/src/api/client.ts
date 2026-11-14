// Typed API client generated from the server's OpenAPI document (ADR-0016).
import createClient, { type Middleware } from "openapi-fetch";
import type { components, paths } from "./schema";

export type Schemas = components["schemas"];

/** An API error with the server's `{error: {code, message}}` body. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;

  constructor(status: number, code: string, message: string) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
  }
}

let csrfToken: string | null = null;
let onUnauthorized: (() => void) | null = null;

/** The CSRF token of the current session (from login or `GET /me`); kept in memory only. */
export function setCsrfToken(token: string | null) {
  csrfToken = token;
}

/** Called when an authenticated request answers 401 (session ended). */
export function setUnauthorizedHandler(handler: (() => void) | null) {
  onUnauthorized = handler;
}

const SAFE_METHODS = new Set(["GET", "HEAD", "OPTIONS"]);

const auth: Middleware = {
  onRequest({ request }) {
    if (!SAFE_METHODS.has(request.method) && csrfToken) {
      request.headers.set("X-CSRF-Token", csrfToken);
    }
    return request;
  },
  onResponse({ request, response }) {
    // A failed login is a 401 too, but it is not a lost session.
    if (response.status === 401 && !new URL(request.url).pathname.endsWith("/auth/login")) {
      onUnauthorized?.();
    }
    return response;
  },
};

export const api = createClient<paths>({
  baseUrl: typeof window === "undefined" ? "http://localhost" : window.location.origin,
  credentials: "same-origin",
  // Resolve `fetch` per call, so tests can stub the global after this module is loaded.
  fetch: (request: Request) => globalThis.fetch(request),
});
api.use(auth);

type Result<T> = { data?: T; error?: unknown; response: Response };

/** Returns the data of a successful call, or throws an [`ApiError`]. */
export async function unwrap<T>(call: Promise<Result<T>>): Promise<T> {
  const { data, error, response } = await call;
  if (response.ok) {
    return data as T;
  }
  const body = error as Partial<Schemas["ErrorBody"]> | undefined;
  throw new ApiError(
    response.status,
    body?.error?.code ?? "internal",
    body?.error?.message ?? `HTTP ${response.status}`,
  );
}

/** A short, user-facing description of any thrown error. */
export function errorText(error: unknown): string {
  if (error instanceof ApiError) return error.message;
  if (error instanceof Error) return error.message;
  return "Something went wrong";
}
