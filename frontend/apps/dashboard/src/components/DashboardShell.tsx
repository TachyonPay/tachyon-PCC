import type { ReactNode } from "react";
import { BrandMark } from "./BrandMark";

export type DashIcon = "pay" | "autopay" | "payout" | "account" | "signup" | "operator";

export interface DashNavItem {
  id: string;
  label: string;
  href?: string;
  group?: string;
  icon?: DashIcon;
}

function Icon({ name }: { name: DashIcon }) {
  const props = {
    className: "dash-nav-icon",
    width: 16,
    height: 16,
    viewBox: "0 0 16 16",
    fill: "none",
    "aria-hidden": true as const,
  };
  switch (name) {
    case "pay":
      return (
        <svg {...props}>
          <rect x="1.5" y="3.5" width="13" height="9" rx="1.5" stroke="currentColor" strokeWidth="1.4" />
          <path d="M1.5 6.5h13" stroke="currentColor" strokeWidth="1.4" />
        </svg>
      );
    case "autopay":
      return (
        <svg {...props}>
          <path d="M3 8a5 5 0 1 1 1.5 3.5" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
          <path d="M8 5.5V8l2 1.2" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
        </svg>
      );
    case "payout":
      return (
        <svg {...props}>
          <path
            d="M8 2.5v11M5 5.5h4.2a2.2 2.2 0 0 1 0 4.4H5h5.2a2.2 2.2 0 0 1 0 4.4H5"
            stroke="currentColor"
            strokeWidth="1.4"
            strokeLinecap="round"
          />
        </svg>
      );
    case "account":
      return (
        <svg {...props}>
          <circle cx="8" cy="6" r="2.2" stroke="currentColor" strokeWidth="1.4" />
          <path d="M3.5 13c.6-2.2 2.3-3.3 4.5-3.3s3.9 1.1 4.5 3.3" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
        </svg>
      );
    case "signup":
      return (
        <svg {...props}>
          <path d="M8 3.5v9M3.5 8h9" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
        </svg>
      );
    case "operator":
      return (
        <svg {...props}>
          <circle cx="8" cy="8" r="5.5" stroke="currentColor" strokeWidth="1.4" />
          <path d="M8 5.5v3.2L10 10" stroke="currentColor" strokeWidth="1.4" strokeLinecap="round" />
        </svg>
      );
  }
}

function groupItems(nav: DashNavItem[]) {
  const groups: { label?: string; items: DashNavItem[] }[] = [];
  for (const item of nav) {
    const last = groups.at(-1);
    if (last && last.label === item.group) last.items.push(item);
    else groups.push(item.group ? { label: item.group, items: [item] } : { items: [item] });
  }
  return groups;
}

export function DashboardShell({
  nav,
  current,
  onNav,
  title,
  actions,
  session,
  footer,
  testMode,
  children,
}: {
  nav: DashNavItem[];
  current: string;
  onNav?: (id: string) => void;
  title: string;
  actions?: ReactNode;
  session?: ReactNode;
  footer?: ReactNode;
  testMode?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="dash-shell">
      {testMode ? (
        <p className="dash-mode" role="status">
          Test mode — dry run, no mainnet transfers
        </p>
      ) : null}
      <div className="dash-app">
      <aside className="dash-sidebar">
        <BrandMark />
        <nav className="dash-sidenav" aria-label="Dashboard">
          {groupItems(nav).map((group, i) => (
            <div key={group.label ?? `g-${i}`} className="dash-nav-group">
              {group.label ? <p className="dash-nav-label">{group.label}</p> : null}
              {group.items.map((item) =>
                item.href ? (
                  <a key={item.id} href={item.href} aria-current={current === item.id ? "page" : undefined}>
                    {item.icon ? <Icon name={item.icon} /> : null}
                    {item.label}
                  </a>
                ) : (
                  <button
                    key={item.id}
                    type="button"
                    aria-current={current === item.id ? "page" : undefined}
                    onClick={() => onNav?.(item.id)}
                  >
                    {item.icon ? <Icon name={item.icon} /> : null}
                    {item.label}
                  </button>
                ),
              )}
            </div>
          ))}
        </nav>
        {footer ? <div className="dash-sidebar-end">{footer}</div> : null}
      </aside>
      <div className="dash-main">
        <header className="dash-top">
          <div className="dash-top-title">
            <h1>{title}</h1>
          </div>
          <div className="dash-top-end">
            {actions}
            {session}
          </div>
        </header>
        <div className="dash-content">{children}</div>
      </div>
      </div>
    </div>
  );
}
