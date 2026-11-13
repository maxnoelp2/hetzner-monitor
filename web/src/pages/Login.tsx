import { useState } from "react";
import type { FormEvent } from "react";
import { Activity } from "lucide-react";
import { ApiError, errorText } from "../api/client";
import { useSession } from "../api/session";
import { Button, ErrorNote, TextField } from "../ui/components";

export function LoginPage() {
  const { login } = useSession();
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  async function submit(e: FormEvent) {
    e.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await login(email.trim(), password);
    } catch (err) {
      if (err instanceof ApiError && err.code === "invalid_credentials") {
        setError("E-mail or password is wrong.");
      } else if (err instanceof ApiError && err.code === "too_many_attempts") {
        setError("Too many failed attempts. Wait a few minutes and try again.");
      } else {
        setError(errorText(err));
      }
      setBusy(false);
    }
  }

  return (
    <div className="auth-page">
      <section className="card auth-card">
        <div className="brand" style={{ fontSize: 18 }}>
          <Activity size={22} strokeWidth={1.5} aria-hidden="true" />
          hetzner-monitor
        </div>
        <h1>Log in</h1>
        <form className="form" onSubmit={submit}>
          <TextField
            label="E-mail"
            type="email"
            autoComplete="username"
            required
            value={email}
            onChange={(e) => setEmail(e.target.value)}
          />
          <TextField
            label="Password"
            type="password"
            autoComplete="current-password"
            required
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
          {error && <ErrorNote>{error}</ErrorNote>}
          <Button type="submit" variant="primary" disabled={busy}>
            {busy ? "Logging in…" : "Log in"}
          </Button>
        </form>
        <p className="small muted">
          The first password is printed by <code>hetzner-monitor bootstrap</code>. A lost password
          is reset with <code>hetzner-monitor users reset-password</code>.
        </p>
      </section>
    </div>
  );
}
