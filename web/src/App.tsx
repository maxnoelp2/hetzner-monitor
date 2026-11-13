import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter, Navigate, Route, Routes } from "react-router";
import { ApiError } from "./api/client";
import { SessionProvider, useSession } from "./api/session";
import { Layout } from "./Layout";
import { AlertDetailPage, AlertsPage } from "./pages/Alerts";
import { ChannelsPage } from "./pages/Channels";
import { LoginPage } from "./pages/Login";
import { NotFoundPage } from "./pages/NotFound";
import { ProjectDetailPage, ProjectsPage } from "./pages/Projects";
import { RulesPage } from "./pages/Rules";
import { SettingsPage } from "./pages/Settings";
import { SilencesPage } from "./pages/Silences";
import { Spinner } from "./ui/components";
import { ToastProvider } from "./ui/toast";

export function makeQueryClient() {
  return new QueryClient({
    defaultOptions: {
      queries: {
        staleTime: 15_000,
        refetchOnWindowFocus: true,
        // Client errors (401, 403, 404 …) do not get better by retrying.
        retry: (count, error) => !(error instanceof ApiError && error.status < 500) && count < 2,
      },
    },
  });
}

/** Routes of the app; exported for tests (wrapped in a MemoryRouter there). */
export function AppRoutes() {
  const { state } = useSession();
  if (state.status === "loading") return <Spinner label="Loading session" />;
  if (state.status === "anonymous") {
    return (
      <Routes>
        <Route path="*" element={<LoginPage />} />
      </Routes>
    );
  }
  return (
    <Routes>
      <Route element={<Layout />}>
        <Route index element={<Navigate to="/alerts" replace />} />
        <Route path="login" element={<Navigate to="/alerts" replace />} />
        <Route path="alerts" element={<AlertsPage />} />
        <Route path="alerts/:id" element={<AlertDetailPage />} />
        <Route path="projects" element={<ProjectsPage />} />
        <Route path="projects/:id" element={<ProjectDetailPage />} />
        <Route path="rules" element={<RulesPage />} />
        <Route path="channels" element={<ChannelsPage />} />
        <Route path="silences" element={<SilencesPage />} />
        <Route path="settings" element={<SettingsPage />} />
        <Route path="*" element={<NotFoundPage />} />
      </Route>
    </Routes>
  );
}

export function App() {
  return (
    <QueryClientProvider client={makeQueryClient()}>
      <ToastProvider>
        <SessionProvider>
          <BrowserRouter>
            <AppRoutes />
          </BrowserRouter>
        </SessionProvider>
      </ToastProvider>
    </QueryClientProvider>
  );
}
