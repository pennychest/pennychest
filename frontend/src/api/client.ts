const BASE_URL = "/api";

// Fired when the server rejects a request because the session is missing or expired.
export const SIGNED_OUT_EVENT = "pennychest:signed-out";

async function request<T>(path: string, options?: RequestInit): Promise<T> {
  const response = await fetch(`${BASE_URL}${path}`, {
    headers: {
      "Content-Type": "application/json",
      ...options?.headers,
    },
    ...options,
  });

  if (!response.ok) {
    if (response.status === 401 && path !== "/auth/login") {
      window.dispatchEvent(new Event(SIGNED_OUT_EVENT));
    }
    const error = await response.json().catch(() => ({ detail: response.statusText }));
    throw new Error(error.detail || `Request failed: ${response.status}`);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return response.json();
}

async function requestFormData<T>(path: string, formData: FormData): Promise<T> {
  const response = await fetch(`${BASE_URL}${path}`, {
    method: "POST",
    body: formData,
  });

  if (!response.ok) {
    if (response.status === 401 && path !== "/auth/login") {
      window.dispatchEvent(new Event(SIGNED_OUT_EVENT));
    }
    const error = await response.json().catch(() => ({ detail: response.statusText }));
    throw new Error(error.detail || `Request failed: ${response.status}`);
  }

  return response.json();
}

// Types
export interface Account {
  id: number;
  name: string;
  full_path: string;
  parent_id: number | null;
  type: "asset" | "liability" | "income" | "expense" | "equity";
  currency: string;
  bank_identifier?: string | null;
  icon?: string | null;
}

export interface Posting {
  id: number;
  transaction_id: number;
  account_id: number;
  amount: string;
  categorised_by_id: number;
  rule_id: number | null;
  account_full_path: string | null;
}

export interface Transaction {
  id: number;
  date: string;
  description: string;
  status: "pending" | "confirmed";
  import_batch_id: number | null;
  created_at: string;
  postings: Posting[];
}

export interface Rule {
  id: number;
  pattern: string;
  match_type_id: number;
  match_type_name: string | null;
  target_account_id: number;
  target_account_path: string | null;
  priority: number;
  source: string | null;
  description: string | null;
}

export interface LookupItem {
  id: number;
  name: string;
}

export interface ReportSummary {
  net_worth: Record<string, number>;
  income: number;
  expenses: number;
  savings_rate: number;
  date_from: string;
  date_to: string;
}

export interface SetupStatus {
  has_accounts: boolean;
  has_rules: boolean;
}

export interface ImportBatch {
  id: number;
  importer_name: string;
  file_name: string | null;
  account_id: number | null;
  account_path: string | null;
  account_name: string | null;
  // Last four characters of the account's bank identifier, if it has one
  account_number_hint: string | null;
  imported_at: string;
  // The period the statement says it covers, when it says (HSBC PDFs do)
  period_start: string | null;
  period_end: string | null;
  transaction_count: number;
  confirmed_count: number;
  pending_count: number;
}

export interface ImportUploadResult {
  batch_id: number;
  file_name: string;
  transaction_count: number;
  categorised_count: number;
  uncategorised_count: number;
  // Categorised by AI straight after the import, if that's turned on
  ai_categorised_count: number;
  // Of those, how many came from the user's own history rather than the AI
  learned_count: number;
  ai_error: string | null;
}

export interface ImportTransaction {
  id: number;
  date: string;
  description: string;
  amount: string;
  status: string;
  account_full_path: string | null;
  target_account_full_path: string | null;
  categorised_by: string | null;
  rule_id: number | null;
  rule_pattern: string | null;
  raw_content: string | null;
  line_number: number | null;
  transfer_peer_id: number | null;
  import_batch_id: number | null;
  source_file_name: string | null;
  // Set when this looks like a transaction already in the ledger, until you delete one or keep it
  duplicate_of: DuplicateOf | null;
  // 0 (normal) to 1 (very unusual), once the insights model has judged it
  unusual_score: number | null;
}

export interface DuplicateOf {
  transaction_id: number;
  date: string;
  description: string;
  source: string; // "imported from <file>" or "added by hand"
}

export interface TransferSuggestion {
  transaction_id: number;
  date: string;
  description: string;
  amount: string;
  account_full_path: string | null;
  days_apart: number;
}

export interface PendingTransfersBalance {
  balance: string;
  unlinked_count: number;
}

export interface ImportBatchDetail {
  id: number;
  importer_name: string;
  source_type_id: number;
  file_path: string | null;
  file_hash: string | null;
  file_name: string | null;
  account_id: number | null;
  imported_at: string;
  transaction_count: number;
  categorised_count: number;
  // When the matching model last looked for duplicates and transfers in it
  matched_at: string | null;
}

export interface ImportReview {
  batch: ImportBatchDetail;
  transactions: ImportTransaction[];
}

export interface RulePreviewResponse {
  matching_transactions: { id: number; description: string }[];
  match_count: number;
  conflicts: RuleConflict[];
}

export interface RuleConflict {
  rule_id: number;
  pattern: string;
  match_type: string;
  priority: number;
  target_account_id: number;
}

export interface ImporterInfo {
  name: string;
  label: string;
  description: string;
  file_types: string[];
}

// What an importer read from a statement that identifies its account
export interface StatementDetection {
  statement_type: "current_account" | "savings" | "credit_card";
  bank_identifier: string;
  label: string;
  name_hint: string | null;
  suggested_account_id: number | null;
  suggested_account_name: string | null;
  period_start: string | null;
  period_end: string | null;
  // Signed strings: positive for asset balances in funds; negative for credit
  // cards with a balance owed.
  opening_balance: string | null;
  closing_balance: string | null;
}

export interface DetectResult {
  importer: string;
  statement: StatementDetection | null;
}

export interface OpeningBalance {
  amount: string;
  date: string;
  transaction_id: number;
}

// Accounts
export const accountsApi = {
  list: () => request<Account[]>("/accounts"),
  get: (id: number) => request<Account>(`/accounts/${id}`),
  create: (data: Omit<Account, "id">) =>
    request<Account>("/accounts", { method: "POST", body: JSON.stringify(data) }),
  update: (id: number, data: Partial<Account>) =>
    request<Account>(`/accounts/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  delete: (id: number) =>
    request<void>(`/accounts/${id}`, { method: "DELETE" }),
  getOpeningBalance: (id: number) =>
    request<OpeningBalance | null>(`/accounts/${id}/opening-balance`),
  setOpeningBalance: (id: number, data: { amount: string; date: string }) =>
    request<OpeningBalance>(`/accounts/${id}/opening-balance`, {
      method: "PUT",
      body: JSON.stringify(data),
    }),
  export: () => request<Record<string, unknown>[]>("/accounts/export"),
  import: (data: Record<string, unknown>[]) =>
    request<{ accounts_created: number }>("/accounts/import", {
      method: "POST",
      body: JSON.stringify(data),
    }),
};

// Transactions
export interface TransactionFilters {
  status?: string;
  account_id?: number;
  // The category and everything beneath it
  category_id?: number;
  batch_id?: number;
  date_from?: string;
  date_to?: string;
  search?: string;
  page?: number;
  per_page?: number;
}

export const transactionsApi = {
  list: (filters?: TransactionFilters) => {
    const params = new URLSearchParams();
    if (filters) {
      Object.entries(filters).forEach(([key, value]) => {
        if (value !== undefined && value !== null && value !== "") {
          params.set(key, String(value));
        }
      });
    }
    const qs = params.toString();
    return request<Transaction[]>(`/transactions${qs ? `?${qs}` : ""}`);
  },
  get: (id: number) => request<Transaction>(`/transactions/${id}`),
  create: (data: {
    date: string;
    description: string;
    status?: string;
    postings: { account_id: number; amount: string; categorised_by_id: number }[];
  }) => request<Transaction>("/transactions", { method: "POST", body: JSON.stringify(data) }),
  update: (id: number, data: Partial<Transaction>) =>
    request<Transaction>(`/transactions/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  delete: (id: number) =>
    request<void>(`/transactions/${id}`, { method: "DELETE" }),
  confirm: (id: number) =>
    request<Transaction>(`/transactions/${id}/confirm`, { method: "POST" }),
  bulkConfirm: (ids: number[]) =>
    request<{ confirmed: number }>("/transactions/bulk-confirm", {
      method: "POST",
      body: JSON.stringify(ids),
    }),
  // How many transactions match, for the same filters as list (paging aside)
  count: (filters?: Omit<TransactionFilters, "page" | "per_page">) => {
    const params = new URLSearchParams();
    Object.entries(filters ?? {}).forEach(([key, value]) => {
      if (value !== undefined && value !== null && value !== "") params.set(key, String(value));
    });
    const qs = params.toString();
    return request<{ count: number }>(`/transactions/count${qs ? `?${qs}` : ""}`);
  },
};

// Rules
export const rulesApi = {
  list: () => request<Rule[]>("/rules"),
  get: (id: number) => request<Rule>(`/rules/${id}`),
  create: (data: Omit<Rule, "id" | "match_type_name" | "target_account_path">) =>
    request<Rule>("/rules", { method: "POST", body: JSON.stringify(data) }),
  update: (id: number, data: Partial<Rule>) =>
    request<Rule>(`/rules/${id}`, { method: "PUT", body: JSON.stringify(data) }),
  delete: (id: number) =>
    request<void>(`/rules/${id}`, { method: "DELETE" }),
  preview: (data: { pattern: string; match_type_id: number; exclude_rule_id?: number }) =>
    request<RulePreviewResponse>("/rules/preview", {
      method: "POST",
      body: JSON.stringify(data),
    }),
  test: (data: { pattern: string; match_type_id: number; descriptions: string[] }) =>
    request<{ matching: string[]; match_count: number; total: number }>("/rules/test", {
      method: "POST",
      body: JSON.stringify(data),
    }),
  export: () => request<Record<string, unknown>[]>("/rules/export"),
  import: (data: Record<string, unknown>[]) =>
    request<{ imported: number; skipped: number }>("/rules/import", {
      method: "POST",
      body: JSON.stringify(data),
    }),
};

export interface AccountBatchSummary {
  id: number;
  file_name: string | null;
  imported_at: string;
  period_start: string | null;
  period_end: string | null;
  transaction_count: number;
  pending_count: number;
}

export interface AccountReview {
  account_id: number;
  account_path: string | null;
  batches: AccountBatchSummary[];
  transactions: ImportTransaction[];
}

// Imports
export const importsApi = {
  listImporters: () => request<ImporterInfo[]>("/imports/importers"),
  // Ask the matching model about the import's duplicates, transfers and card taps
  match: (batchId: number) =>
    request<{ duplicates: number; transfers: number; taps: number }>(
      `/imports/batches/${batchId}/match`,
      { method: "POST" },
    ),
  detect: (file: File) => {
    const formData = new FormData();
    formData.append("file", file);
    return requestFormData<DetectResult>("/imports/detect", formData);
  },
  upload: (file: File, accountId: number, importerName = "csv") => {
    const formData = new FormData();
    formData.append("file", file);
    formData.append("account_id", String(accountId));
    formData.append("importer_name", importerName);
    return requestFormData<ImportUploadResult>("/imports/upload", formData);
  },
  listBatches: () => request<ImportBatch[]>("/imports/batches"),
  getBatch: (batchId: number) => request<ImportReview>(`/imports/batches/${batchId}`),
  confirmAll: (batchId: number) =>
    request<{ confirmed: number }>(`/imports/batches/${batchId}/confirm-all`, {
      method: "POST",
    }),
  reapplyRules: (batchId: number) =>
    request<{ updated: number }>(`/imports/batches/${batchId}/reapply-rules`, {
      method: "POST",
    }),
  categorise: (transactionId: number, targetAccountId: number) =>
    request<{ status: string }>(`/imports/transactions/${transactionId}/categorise?target_account_id=${targetAccountId}`, {
      method: "PUT",
    }),
  deleteBatch: (batchId: number) =>
    request<{ deleted: boolean }>(`/imports/batches/${batchId}`, { method: "DELETE" }),
  getTransferSuggestions: (transactionId: number) =>
    request<TransferSuggestion[]>(`/imports/transactions/${transactionId}/transfer-suggestions`),
  markAsTransfer: (transactionId: number, peerTransactionId: number | null) =>
    request<{ status: string; transaction_id: number; peer_transaction_id: number | null; parked: boolean }>(
      `/imports/transactions/${transactionId}/mark-as-transfer`,
      { method: "POST", body: JSON.stringify({ peer_transaction_id: peerTransactionId }) }
    ),
  markNotDuplicate: (transactionId: number) =>
    request<void>(`/imports/transactions/${transactionId}/not-duplicate`, { method: "POST" }),
  getPendingTransfersBalance: () =>
    request<PendingTransfersBalance>("/imports/transfers/pending-balance"),
  getAccountReview: (accountId: number) =>
    request<AccountReview>(`/imports/account/${accountId}`),
  confirmAllForAccount: (accountId: number) =>
    request<{ confirmed: number }>(`/imports/account/${accountId}/confirm-all`, { method: "POST" }),
  getBatchFileUrl: (batchId: number) => `${BASE_URL}/imports/batches/${batchId}/file`,
};

// Reports
export const reportsApi = {
  summary: (dateFrom?: string, dateTo?: string) => {
    const params = new URLSearchParams();
    if (dateFrom) params.set("date_from", dateFrom);
    if (dateTo) params.set("date_to", dateTo);
    const qs = params.toString();
    return request<ReportSummary>(`/reports/summary${qs ? `?${qs}` : ""}`);
  },
  expensesByCategory: (dateFrom?: string, dateTo?: string, parentPath?: string) => {
    const params = new URLSearchParams();
    if (dateFrom) params.set("date_from", dateFrom);
    if (dateTo) params.set("date_to", dateTo);
    if (parentPath) params.set("parent_path", parentPath);
    const qs = params.toString();
    return request<{ full_path: string; name: string; total: number }[]>(
      `/reports/expenses-by-category${qs ? `?${qs}` : ""}`
    );
  },
};

// Lookups
export const lookupsApi = {
  matchTypes: () => request<LookupItem[]>("/lookup/match-types"),
  categorisationSources: () => request<LookupItem[]>("/lookup/categorisation-sources"),
};

// Setup
export const setupApi = {
  status: () => request<SetupStatus>("/setup/status"),
  seed: () => request<{ accounts_created: number; rules_created: number }>("/setup/seed", { method: "POST" }),
  seedRules: () => request<{ rules_created: number }>("/setup/seed-rules", { method: "POST" }),
};

export const healthApi = {
  check: () => request<{ status: string }>("/health"),
};

// AI configuration
export type AiTask =
  | "taps"
  | "categorise"
  | "rules"
  | "chat"
  | "check_actions"
  | "matching"
  | "insights";

export interface AiProviderField {
  key: string;
  label: string;
  secret: boolean;
  required: boolean;
  placeholder: string;
  is_set: boolean;
  value: string | null;
}

export interface AiModel {
  id: string;
  label: string;
}

// A language model writes text and can do anything; a decision model (like Jev) picks from
// fixed answers with a confidence, and is fast and cheap.
export type AiKind = "llm" | "decision";

// "automatic" runs on new data as it arrives; "on_demand" when you press a button
export type AiMode = "automatic" | "on_demand";

export interface AiProvider {
  id: string;
  label: string;
  kind: AiKind;
  configured: boolean;
  // Why the saved credentials were rejected, the last time they were tried
  problem: string | null;
  fields: AiProviderField[];
  models: AiModel[];
  models_updated_at: string | null;
}

export type ChatScope = "organise" | "transactions";

export interface AiTaskConfig {
  label: string;
  // The kinds of model it can use, and the one it does
  kinds: AiKind[];
  kind: AiKind;
  enabled: boolean;
  // Null for tasks without the choice (rules, chat and checking chat's actions)
  mode: AiMode | null;
  // The model it would use: null when it's off or its kind of model isn't chosen
  provider: string | null;
  model: string | null;
  ready: boolean;
  problem: string | null;
  scopes?: ChatScope[];
}

export interface AiModelChoice {
  provider: string | null;
  model: string | null;
  ready: boolean;
  problem: string | null;
}

export interface AiConfig {
  providers: AiProvider[];
  models: Record<AiKind, AiModelChoice>;
  tasks: Record<AiTask, AiTaskConfig>;
}

// AI
export interface AiCategoriseResult {
  updated: number;
  learned?: number;
  error?: string;
}

// The categoriser that learns from the user's own categorised transactions
export interface LearnedStatus {
  enabled: boolean;
  examples: number;
  categories: number;
  min_examples: number;
  confidence: number;
  ready: boolean;
  check: { coverage: number; accuracy: number | null } | null;
}

export interface RuleSuggestion {
  pattern: string;
  match_type: string;
  match_type_id: number | null;
  target_account_full_path: string;
  target_account_id: number | null;
  priority: number;
  description: string;
}

export interface AiSuggestRulesResult {
  suggestions: RuleSuggestion[];
}

export interface AIRequestLog {
  id: number;
  created_at: string;
  provider: string;
  operation: string;
  request_data: Record<string, unknown> | null;
  response_data: Record<string, unknown> | null;
  duration_ms: number | null;
  error: string | null;
}

export interface AILogsResult {
  logs: AIRequestLog[];
  total: number;
}

export const aiApi = {
  config: () => request<AiConfig>("/ai/config"),
  saveProvider: (providerId: string, values: Record<string, string | null>) =>
    request<void>(`/ai/providers/${providerId}`, {
      method: "PUT",
      body: JSON.stringify({ values }),
    }),
  refreshModels: (providerId: string) =>
    request<{ models: AiModel[]; models_updated_at: string }>(`/ai/providers/${providerId}/models`, {
      method: "POST",
    }),
  learnedStatus: () => request<LearnedStatus>("/ai/learned"),
  setLearned: (enabled: boolean) =>
    request<void>("/ai/learned", { method: "PUT", body: JSON.stringify({ enabled }) }),
  setChatScopes: (scopes: ChatScope[]) =>
    request<void>("/ai/chat/scopes", { method: "PUT", body: JSON.stringify({ scopes }) }),
  chooseModel: (kind: AiKind, provider: string | null, model: string | null) =>
    request<void>(`/ai/models/${kind}`, {
      method: "PUT",
      body: JSON.stringify({ provider, model }),
    }),
  updateTask: (task: AiTask, settings: { enabled?: boolean; kind?: AiKind; mode?: AiMode }) =>
    request<void>(`/ai/tasks/${task}`, { method: "PUT", body: JSON.stringify(settings) }),
  insightsBatch: (batchId: number) =>
    request<{ ok: boolean }>(`/ai/insights/batch/${batchId}`, { method: "POST" }),
  categoriseBatch: (batchId: number) =>
    request<AiCategoriseResult>(`/ai/categorise/batch/${batchId}`, { method: "POST" }),
  categoriseAll: () =>
    request<AiCategoriseResult>("/ai/categorise/all", { method: "POST" }),
  suggestRules: () =>
    request<AiSuggestRulesResult>("/ai/suggest-rules", { method: "POST" }),
  getLogs: (params?: { limit?: number; offset?: number }) => {
    const qs = new URLSearchParams();
    if (params?.limit !== undefined) qs.set("limit", String(params.limit));
    if (params?.offset !== undefined) qs.set("offset", String(params.offset));
    const query = qs.toString();
    return request<AILogsResult>(query ? `/ai/logs?${query}` : "/ai/logs");
  },
};

// Card taps
export type TapStatus = "unmatched" | "reconciled" | "dismissed";

export interface CardTap {
  id: number;
  tapped_on: string;
  merchant: string;
  amount: string;
  currency: string;
  card_name: string | null;
  status: TapStatus;
  account_id: number | null;
  account_full_path: string | null;
  suggested_account_full_path: string | null;
  suggestion_source: "rule" | "learned" | "ai" | null;
  suggestion_confidence: number | null;
  transaction_id: number | null;
  transaction_description: string | null;
  transaction_date: string | null;
  created_at: string;
}

export interface WalletCard {
  card_name: string;
  account_id: number | null;
  account_full_path: string | null;
  tap_count: number;
}

export const tapsApi = {
  list: (status: TapStatus | "all" = "unmatched") =>
    request<CardTap[]>(`/taps?status=${status}`),
  dismiss: (id: number) => request<CardTap>(`/taps/${id}/dismiss`, { method: "POST" }),
  restore: (id: number) => request<CardTap>(`/taps/${id}/restore`, { method: "POST" }),
  delete: (id: number) => request<void>(`/taps/${id}`, { method: "DELETE" }),
  reconcile: () => request<{ reconciled: number }>("/taps/reconcile", { method: "POST" }),
  // Ask the card taps model for a category now
  categorise: (id: number) => request<CardTap>(`/taps/${id}/categorise`, { method: "POST" }),
  cards: () => request<WalletCard[]>("/taps/cards"),
  pairCard: (cardName: string, accountId: number | null) =>
    request<WalletCard>("/taps/cards", {
      method: "PUT",
      body: JSON.stringify({ card_name: cardName, account_id: accountId }),
    }),
  getToken: () => request<{ token: string | null }>("/taps/token"),
  regenerateToken: () => request<{ token: string | null }>("/taps/token", { method: "POST" }),
  clearToken: () => request<void>("/taps/token", { method: "DELETE" }),
};

// Auth
export interface AuthStatus {
  setup_required: boolean;
  authenticated: boolean;
}

export const authApi = {
  status: () => request<AuthStatus>("/auth/status"),
  setup: (password: string) =>
    request<void>("/auth/setup", { method: "POST", body: JSON.stringify({ password }) }),
  login: (password: string) =>
    request<void>("/auth/login", { method: "POST", body: JSON.stringify({ password }) }),
  logout: () => request<void>("/auth/logout", { method: "POST" }),
  changePassword: (currentPassword: string, newPassword: string) =>
    request<void>("/auth/password", {
      method: "POST",
      body: JSON.stringify({ current_password: currentPassword, new_password: newPassword }),
    }),
};

// Budgets
export type BudgetPeriod = "monthly" | "annual";

export interface Budget {
  id: number;
  account_id: number;
  account_full_path: string;
  account_icon: string | null;
  currency: string;
  period: BudgetPeriod;
  period_start: string;
  period_end: string;
  amount: string;
  spent: string;
  unreviewed: string;
  remaining: string;
}

export interface BudgetSuggestion {
  suggested: string;
  months: { month: string; spent: string }[];
}

export const budgetsApi = {
  list: (on?: string) => request<Budget[]>(on ? `/budgets?on=${on}` : "/budgets"),
  create: (accountId: number, amount: string, period: BudgetPeriod) =>
    request<Budget>("/budgets", {
      method: "POST",
      body: JSON.stringify({ account_id: accountId, amount, period }),
    }),
  update: (id: number, changes: { amount?: string; period?: BudgetPeriod }) =>
    request<Budget>(`/budgets/${id}`, { method: "PATCH", body: JSON.stringify(changes) }),
  delete: (id: number) => request<void>(`/budgets/${id}`, { method: "DELETE" }),
  suggestion: (accountId: number, period: BudgetPeriod) =>
    request<BudgetSuggestion>(`/budgets/suggestion?account_id=${accountId}&period=${period}`),
};

// Chat
export interface ChatConversationSummary {
  id: number;
  title: string;
  updated_at: string;
}

export interface ChatToolUse {
  name: string;
  ok: boolean;
  // A change the check model thought the user didn't ask for, so it wasn't made
  held?: boolean;
}

export interface ChatChange {
  id: number;
  summary: string;
  undone: boolean;
}

export interface ChatTurn {
  role: "user" | "assistant";
  content: string;
  tools?: ChatToolUse[];
  changes?: ChatChange[];
}

export type ChatEvent =
  | { event: "conversation"; id: number; title: string }
  | { event: "text"; delta: string }
  | { event: "tool"; name: string; arguments: Record<string, unknown> }
  | { event: "tool_result"; name: string; ok: boolean; held?: boolean }
  | { event: "change"; id: number; summary: string }
  | { event: "message"; content: string }
  | { event: "error"; detail: string }
  | { event: "done" };

async function streamChat(
  message: string,
  conversationId: number | null,
  onEvent: (event: ChatEvent) => void,
): Promise<void> {
  const response = await fetch(`${BASE_URL}/chat/messages`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(
      conversationId === null ? { message } : { message, conversation_id: conversationId },
    ),
  });
  if (!response.ok || !response.body) {
    if (response.status === 401) window.dispatchEvent(new Event(SIGNED_OUT_EVENT));
    const error = await response.json().catch(() => ({ detail: response.statusText }));
    throw new Error(error.detail || `Request failed: ${response.status}`);
  }

  // Server-sent events: blocks of "event: name" and "data: json" lines, separated by a blank line.
  const reader = response.body.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const { done, value } = await reader.read();
    buffer += decoder.decode(value, { stream: !done });
    let boundary = buffer.indexOf("\n\n");
    while (boundary !== -1) {
      const block = buffer.slice(0, boundary);
      buffer = buffer.slice(boundary + 2);
      let name = "";
      let data = "{}";
      for (const line of block.split("\n")) {
        if (line.startsWith("event: ")) name = line.slice(7);
        else if (line.startsWith("data: ")) data = line.slice(6);
      }
      if (name) onEvent({ event: name, ...JSON.parse(data) } as ChatEvent);
      boundary = buffer.indexOf("\n\n");
    }
    if (done) break;
  }
}

export const chatApi = {
  conversations: () => request<ChatConversationSummary[]>("/chat/conversations"),
  conversation: (id: number) =>
    request<{ id: number; title: string; turns: ChatTurn[] }>(`/chat/conversations/${id}`),
  delete: (id: number) => request<void>(`/chat/conversations/${id}`, { method: "DELETE" }),
  send: streamChat,
};

// Changes made through actions (by the chat, the API or agents), with undo
export interface ActionChange {
  id: number;
  created_at: string;
  action: string;
  summary: string;
  source: "chat" | "api" | "mcp" | "import";
  conversation_id: number | null;
  undone_at: string | null;
}

export const changesApi = {
  list: (limit = 50, offset = 0) =>
    request<{ total: number; changes: ActionChange[] }>(`/changes?limit=${limit}&offset=${offset}`),
  undo: (id: number) => request<ActionChange>(`/changes/${id}/undo`, { method: "POST" }),
};

// Personal access tokens for the MCP server at /mcp
export type TokenScope = "organise" | "transactions";

export interface McpToken {
  id: number;
  name: string;
  prefix: string;
  scopes: TokenScope[];
  created_at: string | null;
  last_used_at: string | null;
  kind: "token" | "connector";
}

export const mcpApi = {
  listTokens: () => request<{ tokens: McpToken[] }>("/mcp/tokens"),
  createToken: (name: string, scopes: TokenScope[]) =>
    request<McpToken & { token: string }>("/mcp/tokens", {
      method: "POST",
      body: JSON.stringify({ name, scopes }),
    }),
  revokeToken: (id: number) => request<void>(`/mcp/tokens/${id}`, { method: "DELETE" }),
};

// The OAuth consent page, when a connector such as claude.ai asks to connect
export interface OAuthRequest {
  client_id: string;
  redirect_uri: string;
  state: string | null;
  code_challenge: string | null;
  code_challenge_method: string | null;
  response_type: string;
}

export const oauthApi = {
  info: (clientId: string, redirectUri: string) =>
    request<{ client_name: string; redirect_host: string }>(
      `/oauth/authorize-info?${new URLSearchParams({ client_id: clientId, redirect_uri: redirectUri })}`,
    ),
  approve: (req: OAuthRequest, scopes: TokenScope[]) =>
    request<{ redirect: string }>("/oauth/approve", {
      method: "POST",
      body: JSON.stringify({ ...req, scopes }),
    }),
  deny: (req: OAuthRequest) =>
    request<{ redirect: string }>("/oauth/deny", { method: "POST", body: JSON.stringify(req) }),
};

// Actions: the same typed operations the chat and MCP use. The dashboard reads its figures
// through them so charts and chat answers always agree.
export const actionsApi = {
  call: <T>(name: string, args: Record<string, unknown> = {}) =>
    request<T>(`/actions/${name}`, { method: "POST", body: JSON.stringify(args) }),
};

export interface SpendingSummary {
  total: string;
  unreviewed: string;
  groups: { key: string; total: string; transactions: number }[];
  groups_not_shown: number;
}

export interface CashFlow {
  months: { month: string; income: string; expenses: string; net: string }[];
  income_total: string;
  expenses_total: string;
  unreviewed_expenses: string;
}

export interface NetWorthHistory {
  currencies: Record<string, { date: string; net_worth: string }[]>;
}

export interface RecurringPayment {
  merchant: string;
  kind: "subscription" | "bill" | "other" | "unlabelled";
  cadence: string;
  amount: string;
  yearly_cost: string;
  last_date: string;
}

export interface RecurringPayments {
  payments: RecurringPayment[];
  yearly_total: string;
}

export interface UnusualCharges {
  unusual_charges: {
    transaction_id: number;
    date: string;
    description: string;
    amount: string;
    category: string;
    unusual_score: number;
  }[];
  price_rises: {
    merchant: string;
    kind: string;
    usual_amount: string;
    latest_amount: string;
    latest_date: string;
    increase: string;
  }[];
  unscored_charges: number;
}

// Dashboard layout
export type WidgetType =
  | "stat"
  | "spending_by_category"
  | "spending_trend"
  | "income_vs_expenses"
  | "net_worth"
  | "budgets"
  | "top_merchants"
  | "pending"
  | "unmatched_taps"
  | "subscriptions";

export type Period = "this_month" | "last_month" | "last_3_months" | "last_12_months" | "this_year";

export interface WidgetSettings {
  stat?: "net_worth" | "income" | "expenses" | "savings_rate";
  period?: Period;
  months?: number;
  chart?: "bar" | "donut" | "line";
  categories?: string[];
}

export interface Widget {
  id: string;
  type: WidgetType;
  settings: WidgetSettings;
}

export interface DashboardLayout {
  widgets: Widget[];
}

export const dashboardApi = {
  layout: () => request<DashboardLayout>("/dashboard/layout"),
  save: (layout: DashboardLayout) =>
    request<DashboardLayout>("/dashboard/layout", { method: "PUT", body: JSON.stringify(layout) }),
  reset: () => request<DashboardLayout>("/dashboard/layout", { method: "DELETE" }),
};

// Exporting data
export interface ExporterInfo {
  name: string;
  label: string;
  description: string;
  file_extension: string;
}

export interface ExportInfo {
  engine: string;
  database_backup: boolean;
  size_bytes: number | null;
  // The installed exporter plugins (e.g. Beancount)
  exporters: ExporterInfo[];
}

export const exportApi = {
  info: () => request<ExportInfo>("/export"),
  // Plain links, so the browser downloads the file itself
  databaseUrl: `${BASE_URL}/export/database`,
  exporterUrl: (name: string) => `${BASE_URL}/export/${encodeURIComponent(name)}`,
};

// Plugins
export interface RepositoryPlugin {
  package: string;
  name: string;
  description: string;
  // The version the repository offers
  version: string;
  installed_version: string | null;
  // Built into the image, so it can't be removed here
  built_in: boolean;
}

export interface PluginRepository {
  url: string;
  name: string;
  official: boolean;
  error: string | null;
  plugins: RepositoryPlugin[];
}

export interface PluginsInfo {
  repositories: PluginRepository[];
  // Installed from a repository that's since been removed, or can't be read
  other_installed: { package: string; version: string }[];
  can_add_repositories: boolean;
}

export const pluginsApi = {
  list: () => request<PluginsInfo>("/plugins"),
  addRepository: (url: string) =>
    request<void>("/plugins/repositories", { method: "POST", body: JSON.stringify({ url }) }),
  removeRepository: (url: string) =>
    request<void>(`/plugins/repositories?url=${encodeURIComponent(url)}`, { method: "DELETE" }),
  install: (repository: string, pkg: string) =>
    request<void>("/plugins/install", {
      method: "POST",
      body: JSON.stringify({ repository, package: pkg }),
    }),
  uninstall: (pkg: string) =>
    request<void>("/plugins/uninstall", { method: "POST", body: JSON.stringify({ package: pkg }) }),
};
