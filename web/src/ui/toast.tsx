// Transient notifications ("Channel saved", "Test failed: timeout").
import { createContext, useCallback, useContext, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { X } from "lucide-react";

type ToastTone = "info" | "success" | "warning" | "danger";
type ToastItem = { id: number; tone: ToastTone; title: string; detail?: string };
type Notify = (tone: ToastTone, title: string, detail?: string) => void;

const ToastContext = createContext<Notify>(() => {});
const COLORS: Record<ToastTone, string> = {
  info: "var(--aqua-300)",
  success: "var(--success)",
  warning: "var(--warning)",
  danger: "var(--danger)",
};
let next = 1;

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const dismiss = useCallback(
    (id: number) => setItems((all) => all.filter((t) => t.id !== id)),
    [],
  );
  const notify = useCallback<Notify>(
    (tone, title, detail) => {
      const id = next++;
      setItems((all) => [...all.slice(-3), { id, tone, title, detail }]);
      window.setTimeout(() => dismiss(id), tone === "danger" ? 8000 : 4000);
    },
    [dismiss],
  );
  const value = useMemo(() => notify, [notify]);
  return (
    <ToastContext.Provider value={value}>
      {children}
      <div className="toasts" aria-live="polite">
        {items.map((t) => (
          <div key={t.id} className="toast" role="status">
            <span className="tone" style={{ background: COLORS[t.tone] }} aria-hidden="true" />
            <div className="body">
              <div>{t.title}</div>
              {t.detail && <div className="detail">{t.detail}</div>}
            </div>
            <button
              type="button"
              className="btn btn-ghost btn-icon btn-sm"
              aria-label="Dismiss"
              onClick={() => dismiss(t.id)}
            >
              <X size={14} strokeWidth={1.5} />
            </button>
          </div>
        ))}
      </div>
    </ToastContext.Provider>
  );
}

export function useToast(): Notify {
  return useContext(ToastContext);
}
