import { NavLink, Outlet } from "react-router";
import {
  Activity,
  Bell,
  BellOff,
  FolderKanban,
  LogOut,
  Radio,
  Settings,
  SlidersHorizontal,
} from "lucide-react";
import { useProfile, useSession } from "./api/session";
import { IconButton } from "./ui/components";
import { useToast } from "./ui/toast";
import { errorText } from "./api/client";

const LINKS = [
  { to: "/alerts", label: "Alerts", icon: Bell },
  { to: "/projects", label: "Projects", icon: FolderKanban },
  { to: "/rules", label: "Rules", icon: SlidersHorizontal },
  { to: "/channels", label: "Channels", icon: Radio },
  { to: "/silences", label: "Silences", icon: BellOff },
  { to: "/settings", label: "Settings", icon: Settings },
];

export function Layout() {
  const profile = useProfile();
  const { logout } = useSession();
  const notify = useToast();
  return (
    <>
      <header className="app-header">
        <a className="brand" href="/alerts">
          <Activity size={20} strokeWidth={1.5} aria-hidden="true" />
          hetzner-monitor
        </a>
        <nav className="nav" aria-label="Main">
          {LINKS.map(({ to, label, icon: Icon }) => (
            <NavLink key={to} to={to}>
              <Icon size={16} strokeWidth={1.5} aria-hidden="true" />
              {label}
            </NavLink>
          ))}
        </nav>
        <div className="header-end">
          <span className="small muted hide-mobile" title={profile.user.email}>
            {profile.org.name}
          </span>
          <IconButton
            label="Log out"
            onClick={() =>
              logout().catch((e: unknown) => notify("danger", "Logout failed", errorText(e)))
            }
          >
            <LogOut size={16} strokeWidth={1.5} />
          </IconButton>
        </div>
      </header>
      <main className="main">
        <Outlet />
      </main>
    </>
  );
}
