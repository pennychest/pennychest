import {
  createRouter,
  createRoute,
  createRootRoute,
  Outlet,
} from "@tanstack/react-router";
import { AuthGate } from "./components/AuthGate";
import { Layout } from "./components/Layout";
import { DashboardPage } from "./pages/DashboardPage";
import { TransactionsPage } from "./pages/TransactionsPage";
import { AccountsPage } from "./pages/AccountsPage";
import { RulesPage } from "./pages/RulesPage";
import { ImportPage } from "./pages/ImportPage";
import { ImportReviewPage } from "./pages/ImportReviewPage";
import { AccountReviewPage } from "./pages/AccountReviewPage";
import { SettingsPage } from "./pages/SettingsPage";
import { BudgetsPage } from "./pages/BudgetsPage";
import { AILogsPage } from "./pages/AILogsPage";
import { TapsPage } from "./pages/TapsPage";
import { ChatPage } from "./pages/ChatPage";
import { OAuthConsentPage } from "./pages/OAuthConsentPage";

const rootRoute = createRootRoute({
  component: () => (
    <AuthGate>
      <Layout>
        <Outlet />
      </Layout>
    </AuthGate>
  ),
});

const indexRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/",
  component: DashboardPage,
});

const transactionsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/transactions",
  component: TransactionsPage,
});

const accountsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/accounts",
  component: AccountsPage,
});

const rulesRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/rules",
  component: RulesPage,
});

const importsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/imports",
  component: ImportPage,
});

const importReviewRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/imports/$batchId",
  component: ImportReviewPage,
});

const accountReviewRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/imports/account/$accountId",
  component: AccountReviewPage,
});

const settingsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings",
  component: SettingsPage,
});

const budgetsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/budgets",
  component: BudgetsPage,
});

const tapsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/imports/taps",
  component: TapsPage,
});

const chatRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/chat",
  component: ChatPage,
});

const aiLogsRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/settings/ai-logs",
  component: AILogsPage,
});

const oauthConsentRoute = createRoute({
  getParentRoute: () => rootRoute,
  path: "/oauth/authorize",
  component: OAuthConsentPage,
});

const routeTree = rootRoute.addChildren([
  indexRoute,
  transactionsRoute,
  accountsRoute,
  rulesRoute,
  importsRoute,
  importReviewRoute,
  accountReviewRoute,
  settingsRoute,
  budgetsRoute,
  tapsRoute,
  aiLogsRoute,
  chatRoute,
  oauthConsentRoute,
]);

export const router = createRouter({ routeTree });

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
