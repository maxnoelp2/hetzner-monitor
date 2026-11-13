// Session state: who is logged in, the CSRF token, login and logout.
import { createContext, useCallback, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import {
  api,
  ApiError,
  setCsrfToken,
  setUnauthorizedHandler,
  unwrap,
  type Schemas,
} from "./client";

export type Profile = Schemas["Profile"];

type SessionState =
  { status: "loading" } | { status: "anonymous" } | { status: "authenticated"; profile: Profile };

type SessionContext = {
  state: SessionState;
  login: (email: string, password: string) => Promise<void>;
  logout: () => Promise<void>;
};

const Context = createContext<SessionContext | null>(null);

export function SessionProvider({ children }: { children: ReactNode }) {
  const [state, setState] = useState<SessionState>({ status: "loading" });
  const queryClient = useQueryClient();

  const signedOut = useCallback(() => {
    setCsrfToken(null);
    queryClient.clear();
    setState({ status: "anonymous" });
  }, [queryClient]);

  useEffect(() => {
    setUnauthorizedHandler(signedOut);
    let cancelled = false;
    unwrap(api.GET("/api/v1/me"))
      .then((profile) => {
        if (cancelled) return;
        setCsrfToken(profile.csrf_token);
        setState({ status: "authenticated", profile });
      })
      .catch((error: unknown) => {
        if (cancelled) return;
        if (!(error instanceof ApiError) || error.status !== 401) {
          console.warn("could not load the session", error);
        }
        setState({ status: "anonymous" });
      });
    return () => {
      cancelled = true;
      setUnauthorizedHandler(null);
    };
  }, [signedOut]);

  const login = useCallback(async (email: string, password: string) => {
    const profile = await unwrap(api.POST("/api/v1/auth/login", { body: { email, password } }));
    setCsrfToken(profile.csrf_token);
    setState({ status: "authenticated", profile });
  }, []);

  const logout = useCallback(async () => {
    try {
      await unwrap(api.POST("/api/v1/auth/logout"));
    } finally {
      signedOut();
    }
  }, [signedOut]);

  const value = useMemo(() => ({ state, login, logout }), [state, login, logout]);
  return <Context.Provider value={value}>{children}</Context.Provider>;
}

export function useSession(): SessionContext {
  const ctx = useContext(Context);
  if (!ctx) throw new Error("useSession outside SessionProvider");
  return ctx;
}

/** The logged-in profile; only valid below the authenticated part of the app. */
export function useProfile(): Profile {
  const { state } = useSession();
  if (state.status !== "authenticated") throw new Error("not authenticated");
  return state.profile;
}
