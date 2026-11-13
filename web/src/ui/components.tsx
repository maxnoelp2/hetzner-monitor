// Small components re-implemented from the owner's design system (ADR-0016), styled by ui.css.
import { useEffect, useId, useRef } from "react";
import type {
  ButtonHTMLAttributes,
  InputHTMLAttributes,
  ReactNode,
  SelectHTMLAttributes,
  TextareaHTMLAttributes,
} from "react";
import { AlertTriangle, Inbox, X } from "lucide-react";

type ButtonVariant = "primary" | "secondary" | "outline" | "ghost" | "danger";

export function Button({
  variant = "outline",
  size,
  icon,
  className,
  children,
  type = "button",
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & {
  variant?: ButtonVariant;
  size?: "sm";
  icon?: ReactNode;
}) {
  const cls = ["btn", `btn-${variant}`, size === "sm" && "btn-sm", className]
    .filter(Boolean)
    .join(" ");
  return (
    <button type={type} className={cls} {...rest}>
      {icon}
      {children}
    </button>
  );
}

export function IconButton({
  label,
  children,
  className,
  ...rest
}: ButtonHTMLAttributes<HTMLButtonElement> & { label: string }) {
  return (
    <button
      type="button"
      className={["btn btn-ghost btn-icon", className].filter(Boolean).join(" ")}
      aria-label={label}
      title={label}
      {...rest}
    >
      {children}
    </button>
  );
}

export type Tone = "aqua" | "pink" | "neutral" | "success" | "warning" | "danger" | "info";

export function Badge({
  tone = "neutral",
  dot,
  children,
}: {
  tone?: Tone;
  dot?: boolean;
  children: ReactNode;
}) {
  return (
    <span className={`badge badge-${tone}`}>
      {dot && <span className="dot" aria-hidden="true" />}
      {children}
    </span>
  );
}

export function Card({
  title,
  eyebrow,
  actions,
  children,
}: {
  title?: ReactNode;
  eyebrow?: ReactNode;
  actions?: ReactNode;
  children?: ReactNode;
}) {
  return (
    <section className="card">
      {(title || actions || eyebrow) && (
        <div className="row">
          <div className="stack" style={{ gap: 2 }}>
            {eyebrow && <div className="overline">{eyebrow}</div>}
            {title && <h2 className="card-title">{title}</h2>}
          </div>
          {actions && (
            <>
              <span className="spacer" />
              <div className="row">{actions}</div>
            </>
          )}
        </div>
      )}
      {children}
    </section>
  );
}

export function PageHead({
  title,
  eyebrow,
  description,
  actions,
}: {
  title: ReactNode;
  eyebrow?: ReactNode;
  description?: ReactNode;
  actions?: ReactNode;
}) {
  return (
    <div className="page-head">
      <div>
        {eyebrow && <div className="overline">{eyebrow}</div>}
        <h1>{title}</h1>
        {description && <p className="muted">{description}</p>}
      </div>
      {actions && <div className="row">{actions}</div>}
    </div>
  );
}

type FieldProps = { label: string; hint?: ReactNode; error?: string | null };

function Field({
  label,
  hint,
  error,
  id,
  children,
}: FieldProps & { id: string; children: ReactNode }) {
  return (
    <div className="field">
      <label className="label" htmlFor={id}>
        {label}
      </label>
      {children}
      {error ? (
        <span className="error" id={`${id}-msg`}>
          {error}
        </span>
      ) : (
        hint && (
          <span className="hint" id={`${id}-msg`}>
            {hint}
          </span>
        )
      )}
    </div>
  );
}

export function TextField({
  label,
  hint,
  error,
  ...rest
}: FieldProps & InputHTMLAttributes<HTMLInputElement>) {
  const id = useId();
  return (
    <Field label={label} hint={hint} error={error} id={id}>
      <input
        id={id}
        className="input"
        aria-invalid={error ? true : undefined}
        aria-describedby={error || hint ? `${id}-msg` : undefined}
        {...rest}
      />
    </Field>
  );
}

export function SelectField({
  label,
  hint,
  error,
  children,
  ...rest
}: FieldProps & SelectHTMLAttributes<HTMLSelectElement>) {
  const id = useId();
  return (
    <Field label={label} hint={hint} error={error} id={id}>
      <select
        id={id}
        className="select"
        aria-describedby={error || hint ? `${id}-msg` : undefined}
        {...rest}
      >
        {children}
      </select>
    </Field>
  );
}

export function TextAreaField({
  label,
  hint,
  error,
  mono,
  ...rest
}: FieldProps & TextareaHTMLAttributes<HTMLTextAreaElement> & { mono?: boolean }) {
  const id = useId();
  return (
    <Field label={label} hint={hint} error={error} id={id}>
      <textarea
        id={id}
        className={mono ? "textarea mono" : "textarea"}
        aria-invalid={error ? true : undefined}
        aria-describedby={error || hint ? `${id}-msg` : undefined}
        {...rest}
      />
    </Field>
  );
}

export function Switch({
  checked,
  onChange,
  label,
  disabled,
}: {
  checked: boolean;
  onChange: (checked: boolean) => void;
  label: ReactNode;
  disabled?: boolean;
}) {
  return (
    <label className="switch">
      <input
        type="checkbox"
        role="switch"
        checked={checked}
        disabled={disabled}
        onChange={(e) => onChange(e.target.checked)}
      />
      <span className="track" aria-hidden="true" />
      <span>{label}</span>
    </label>
  );
}

/** A modal built on `<dialog>` (focus trap, Escape and backdrop handled by the browser). */
export function Dialog({
  open,
  onClose,
  title,
  children,
}: {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  children: ReactNode;
}) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    if (open && !el.open) {
      if (typeof el.showModal === "function") el.showModal();
      else el.setAttribute("open", "");
    } else if (!open && el.open) {
      if (typeof el.close === "function") el.close();
      else el.removeAttribute("open");
    }
  }, [open]);
  return (
    <dialog
      ref={ref}
      className="dialog"
      onClose={onClose}
      onCancel={(e) => {
        e.preventDefault();
        onClose();
      }}
    >
      {open && (
        <>
          <div className="dialog-head">
            <h2>{title}</h2>
            <IconButton label="Close" onClick={onClose}>
              <X size={16} strokeWidth={1.5} />
            </IconButton>
          </div>
          {children}
        </>
      )}
    </dialog>
  );
}

export function Spinner({ label = "Loading" }: { label?: string }) {
  return (
    <div className="center" role="status">
      <span className="spinner" aria-hidden="true" />
      <span className="visually-hidden">{label}</span>
    </div>
  );
}

export function Empty({
  title,
  children,
  icon,
}: {
  title: string;
  children?: ReactNode;
  icon?: ReactNode;
}) {
  return (
    <div className="empty">
      <span className="icon">{icon ?? <Inbox size={24} strokeWidth={1.5} />}</span>
      <h3>{title}</h3>
      {children}
    </div>
  );
}

export function ErrorNote({ children }: { children: ReactNode }) {
  return (
    <div className="callout callout-danger" role="alert">
      <AlertTriangle size={16} strokeWidth={1.5} color="var(--danger)" />
      <div>{children}</div>
    </div>
  );
}
