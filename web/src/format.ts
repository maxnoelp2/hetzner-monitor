// Display helpers shared by the pages.
import type { Schemas } from "./api/client";
import type { Tone } from "./ui/components";

export type Severity = Schemas["Severity"];
export type AlertState = Schemas["AlertState"];
export type CheckType = Schemas["CheckType"];
export type ResourceKind = Schemas["ResourceKind"];

export const SEVERITIES: Severity[] = ["info", "warning", "critical"];

export function severityTone(s: Severity): Tone {
  return s === "critical" ? "danger" : s === "warning" ? "warning" : "info";
}

export function alertStateTone(s: AlertState): Tone {
  return s === "firing" ? "pink" : s === "pending" ? "warning" : "success";
}

export function projectStatusTone(s: Schemas["ProjectStatus"]): Tone {
  return s === "active" ? "success" : s === "paused" ? "neutral" : "danger";
}

export const CHECK_LABELS: Record<CheckType, string> = {
  server_status: "Server status",
  resource_lifecycle: "Resource created/deleted",
  lb_target_health: "Load balancer targets",
  traffic_quota: "Traffic quota",
  hetzner_api: "Hetzner API",
  cpu: "CPU",
  network: "Network",
  disk_io: "Disk I/O",
  certificate_expiry: "Certificate expiry",
  deprecation: "Deprecation",
  hygiene_backups: "Backups off",
  hygiene_protection: "Protection off",
  hygiene_orphans: "Orphaned resources",
  cost_change: "Cost change",
};

export const KIND_LABELS: Record<ResourceKind, string> = {
  server: "Servers",
  load_balancer: "Load balancers",
  volume: "Volumes",
  certificate: "Certificates",
  floating_ip: "Floating IPs",
  primary_ip: "Primary IPs",
};

const dateFmt = new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" });

/** "13 Nov 2026, 10:42" in the viewer's locale and time zone. */
export function dateTime(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "—" : dateFmt.format(d);
}

const rtf = new Intl.RelativeTimeFormat(undefined, { numeric: "auto" });
const UNITS: [Intl.RelativeTimeFormatUnit, number][] = [
  ["day", 86400],
  ["hour", 3600],
  ["minute", 60],
  ["second", 1],
];

/** "3 minutes ago" / "in 2 hours", relative to `now`. */
export function relTime(iso: string | null | undefined, now: Date = new Date()): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  const diff = Math.round((d.getTime() - now.getTime()) / 1000);
  for (const [unit, secs] of UNITS) {
    if (Math.abs(diff) >= secs || unit === "second") {
      return rtf.format(Math.round(diff / secs), unit);
    }
  }
  return "—";
}

/** Formats seconds as "10 min", "2 h", "1 d". */
export function duration(seconds: number): string {
  if (seconds % 86400 === 0 && seconds >= 86400) return `${seconds / 86400} d`;
  if (seconds % 3600 === 0 && seconds >= 3600) return `${seconds / 3600} h`;
  if (seconds % 60 === 0 && seconds >= 60) return `${seconds / 60} min`;
  return `${seconds} s`;
}

export function money(amount: number, currency: string): string {
  return new Intl.NumberFormat(undefined, { style: "currency", currency }).format(amount);
}
