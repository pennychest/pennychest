import { Link, useLocation } from "@tanstack/react-router";
import { LayoutDashboard, ArrowRightLeft, Landmark, Upload, PiggyBank, Settings, MessageCircle } from "lucide-react";
import { cn } from "../lib/utils";

const navItems = [
  { to: "/transactions", label: "Transactions", icon: ArrowRightLeft },
  { to: "/imports", label: "Import", icon: Upload },
  { to: "/", label: "Dashboard", icon: LayoutDashboard },
  { to: "/accounts", label: "Accounts", icon: Landmark },
  { to: "/budgets", label: "Budgets", icon: PiggyBank },
] as const;

export function Layout({ children }: { children: React.ReactNode }) {
  const location = useLocation();

  function isActive(to: string) {
    return to === "/" ? location.pathname === "/" : location.pathname.startsWith(to);
  }

  return (
    <div className="min-h-screen bg-background">
      {/* Top header — desktop nav */}
      <header className="sticky top-0 z-40 border-b bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60">
        <div className="mx-auto flex h-14 max-w-7xl items-center px-4">
          <Link to="/" className="flex items-center gap-2 font-semibold text-lg mr-8 shrink-0">
            <img src="/logo.png" alt="PennyChest" className="h-8 w-8" />
            <span>
              <span style={{ color: "#D4A017" }}>Penny</span>
              <span style={{ color: "#2A6B2A" }}>Chest</span>
            </span>
          </Link>
          {/* Desktop nav: full labels */}
          <nav className="hidden sm:flex items-center gap-1">
            {navItems.map(({ to, label, icon: Icon }) => (
              <Link
                key={to}
                to={to}
                className={cn(
                  "flex items-center gap-2 rounded-md px-3 py-2 text-sm font-medium transition-colors hover:bg-accent",
                  isActive(to) ? "bg-accent text-accent-foreground" : "text-muted-foreground"
                )}
              >
                <Icon className="h-4 w-4" />
                {label}
              </Link>
            ))}
          </nav>
          {/* Right-aligned icons */}
          <div className="ml-auto flex items-center gap-1">
            <Link
              to="/chat"
              className={cn(
                "flex items-center justify-center rounded-md p-2 transition-colors hover:bg-accent",
                isActive("/chat") ? "bg-accent text-accent-foreground" : "text-muted-foreground"
              )}
              aria-label="Chat"
              title="Chat"
            >
              <MessageCircle className="h-5 w-5" />
            </Link>
            <Link
              to="/settings"
              className={cn(
                "flex items-center justify-center rounded-md p-2 transition-colors hover:bg-accent",
                isActive("/settings") ? "bg-accent text-accent-foreground" : "text-muted-foreground"
              )}
              aria-label="Settings"
            >
              <Settings className="h-5 w-5" />
            </Link>
          </div>
        </div>
      </header>

      {/* Page content — extra bottom padding on mobile so content clears the nav bar */}
      <main className="mx-auto max-w-7xl px-4 py-6 pb-[calc(6rem+env(safe-area-inset-bottom))] sm:pb-6">{children}</main>

      {/* Mobile bottom nav */}
      <nav className="sm:hidden fixed bottom-0 inset-x-0 z-40 border-t bg-background/95 backdrop-blur supports-[backdrop-filter]:bg-background/60 flex pb-[env(safe-area-inset-bottom)] px-[max(0.5rem,env(safe-area-inset-left))]">
        {navItems.map(({ to, label, icon: Icon }) => (
          <Link
            key={to}
            to={to}
            className={cn(
              "flex flex-1 flex-col items-center gap-1 py-2 text-xs font-medium transition-colors",
              isActive(to) ? "text-foreground" : "text-muted-foreground"
            )}
          >
            <Icon className={cn("h-5 w-5", isActive(to) && "text-primary")} />
            {label}
          </Link>
        ))}
      </nav>
    </div>
  );
}
