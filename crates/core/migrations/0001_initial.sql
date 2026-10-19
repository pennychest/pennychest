-- PennyChest's schema.
--
--   * Amounts of money are INTEGER ten-thousandths of a unit (see money.rs), not floating point.
--   * Every table is STRICT, so SQLite refuses a value of the wrong type, e.g. a float amount.
--   * Account types are stored as their values ('asset').
--   * JSON columns must hold valid JSON, booleans 0 or 1, and enumerations one of their values.
--   * Deleting a transaction deletes its postings, and deleting an import batch its raw rows,
--     in the database rather than only in application code.
--
-- Dates are TEXT 'YYYY-MM-DD' and times TEXT 'YYYY-MM-DD HH:MM:SS' in UTC, as CURRENT_TIMESTAMP
-- writes them. Expiry times are INTEGER Unix seconds.

-- Lookup tables. Their ids are referenced by name in code, and seeded below in a fixed order
-- so the ids match too.

CREATE TABLE budget_periods (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE match_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE categorisation_sources (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
) STRICT;

CREATE TABLE import_source_types (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE
) STRICT;

INSERT INTO budget_periods (id, name) VALUES (1, 'monthly'), (2, 'annual');
INSERT INTO match_types (id, name) VALUES (1, 'substring'), (2, 'prefix'), (3, 'regex');
INSERT INTO categorisation_sources (id, name) VALUES
    (1, 'rule'), (2, 'manual'), (3, 'import_default'), (4, 'ai'), (5, 'transfer'), (6, 'learned');
INSERT INTO import_source_types (id, name) VALUES (1, 'file'), (2, 'api');

-- The ledger

CREATE TABLE accounts (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    full_path TEXT NOT NULL UNIQUE,
    parent_id INTEGER REFERENCES accounts (id),
    type TEXT NOT NULL CHECK (type IN ('asset', 'liability', 'income', 'expense', 'equity')),
    currency TEXT NOT NULL DEFAULT 'GBP',
    bank_identifier TEXT,
    icon TEXT
) STRICT;

CREATE INDEX ix_accounts_parent_id ON accounts (parent_id);

CREATE TABLE import_batches (
    id INTEGER PRIMARY KEY,
    importer_name TEXT NOT NULL,
    source_type_id INTEGER NOT NULL REFERENCES import_source_types (id),
    file_path TEXT,
    file_hash TEXT,
    file_name TEXT,
    account_id INTEGER REFERENCES accounts (id),
    imported_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    period_start TEXT,
    period_end TEXT,
    opening_balance INTEGER,
    closing_balance INTEGER,
    matched_at TEXT
) STRICT;

CREATE TABLE transactions (
    id INTEGER PRIMARY KEY,
    date TEXT NOT NULL,
    description TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'confirmed')),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    import_batch_id INTEGER REFERENCES import_batches (id) ON DELETE SET NULL,
    import_metadata TEXT CHECK (import_metadata IS NULL OR json_valid(import_metadata)),
    currency TEXT NOT NULL DEFAULT 'GBP',
    transfer_peer_id INTEGER REFERENCES transactions (id) ON DELETE SET NULL,
    -- Set at import when this looks like a transaction already in the ledger, until the user
    -- deletes one of them or says it isn't a duplicate.
    duplicate_of_id INTEGER REFERENCES transactions (id) ON DELETE SET NULL,
    -- How unusual this charge is for its merchant and category, from 0 (normal) to 1.
    unusual_score REAL CHECK (unusual_score IS NULL OR unusual_score BETWEEN 0 AND 1)
) STRICT;

CREATE UNIQUE INDEX uq_transfer_peer_id ON transactions (transfer_peer_id)
    WHERE transfer_peer_id IS NOT NULL;
CREATE INDEX ix_transactions_date ON transactions (date);
CREATE INDEX ix_transactions_import_batch_id ON transactions (import_batch_id);

CREATE TABLE rules (
    id INTEGER PRIMARY KEY,
    pattern TEXT NOT NULL,
    match_type_id INTEGER NOT NULL REFERENCES match_types (id),
    target_account_id INTEGER NOT NULL REFERENCES accounts (id),
    priority INTEGER NOT NULL DEFAULT 0,
    source TEXT DEFAULT 'manual',
    description TEXT
) STRICT;

-- Every transaction's postings sum to zero. The code guarantees it: a transaction can only be
-- built from balanced postings. SQLite can't defer a check to the end of a transaction, so
-- there is no trigger for it here.
CREATE TABLE postings (
    id INTEGER PRIMARY KEY,
    transaction_id INTEGER NOT NULL REFERENCES transactions (id) ON DELETE CASCADE,
    account_id INTEGER NOT NULL REFERENCES accounts (id),
    amount INTEGER NOT NULL,
    categorised_by_id INTEGER NOT NULL REFERENCES categorisation_sources (id),
    rule_id INTEGER REFERENCES rules (id) ON DELETE SET NULL
) STRICT;

CREATE INDEX ix_postings_transaction_id ON postings (transaction_id);
CREATE INDEX ix_postings_account_id ON postings (account_id);

CREATE TABLE raw_import_rows (
    id INTEGER PRIMARY KEY,
    batch_id INTEGER NOT NULL REFERENCES import_batches (id) ON DELETE CASCADE,
    raw_content TEXT NOT NULL,
    line_number INTEGER,
    transaction_id INTEGER REFERENCES transactions (id) ON DELETE SET NULL
) STRICT;

CREATE INDEX ix_raw_import_rows_batch_id ON raw_import_rows (batch_id);

CREATE TABLE budgets (
    id INTEGER PRIMARY KEY,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    period_id INTEGER NOT NULL REFERENCES budget_periods (id),
    amount INTEGER NOT NULL CHECK (amount > 0),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (account_id, period_id)
) STRICT;

CREATE TABLE csv_templates (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE,
    header_key TEXT NOT NULL,
    columns TEXT NOT NULL CHECK (json_valid(columns)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE INDEX ix_csv_templates_header_key ON csv_templates (header_key);

-- Settings and sign-in

CREATE TABLE app_settings (
    key TEXT PRIMARY KEY,
    value TEXT
) STRICT;

CREATE TABLE auth_sessions (
    id INTEGER PRIMARY KEY,
    token_hash TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_at INTEGER NOT NULL
) STRICT;

-- AI

CREATE TABLE ai_request_logs (
    id INTEGER PRIMARY KEY,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    provider TEXT NOT NULL,
    operation TEXT NOT NULL,
    request_data TEXT CHECK (request_data IS NULL OR json_valid(request_data)),
    response_data TEXT CHECK (response_data IS NULL OR json_valid(response_data)),
    duration_ms INTEGER,
    error TEXT
) STRICT;

CREATE INDEX ix_ai_request_logs_created_at ON ai_request_logs (created_at);

CREATE TABLE merchant_labels (
    merchant_key TEXT PRIMARY KEY,
    kind TEXT NOT NULL CHECK (kind IN ('subscription', 'bill', 'other')),
    confidence REAL,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE chat_conversations (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE chat_messages (
    id INTEGER PRIMARY KEY,
    conversation_id INTEGER NOT NULL REFERENCES chat_conversations (id) ON DELETE CASCADE,
    role TEXT NOT NULL,
    content TEXT,
    tool_calls TEXT CHECK (tool_calls IS NULL OR json_valid(tool_calls)),
    tool_call_id TEXT,
    tool_name TEXT,
    is_error INTEGER NOT NULL DEFAULT 0 CHECK (is_error IN (0, 1)),
    -- The provider's own form of the message, so a conversation carries on intact.
    raw TEXT CHECK (raw IS NULL OR json_valid(raw)),
    raw_provider TEXT,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE INDEX ix_chat_messages_conversation_id ON chat_messages (conversation_id);

-- Changes made through actions (chat, MCP or the API), each with what it takes to undo it.
CREATE TABLE action_changes (
    id INTEGER PRIMARY KEY,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    action TEXT NOT NULL,
    arguments TEXT CHECK (arguments IS NULL OR json_valid(arguments)),
    summary TEXT NOT NULL,
    undo TEXT NOT NULL CHECK (json_valid(undo)),
    source TEXT NOT NULL CHECK (source IN ('chat', 'api', 'mcp', 'import')),
    conversation_id INTEGER REFERENCES chat_conversations (id) ON DELETE SET NULL,
    chat_message_id INTEGER REFERENCES chat_messages (id) ON DELETE SET NULL,
    undone_at TEXT
) STRICT;

-- Card taps

CREATE TABLE wallet_cards (
    id INTEGER PRIMARY KEY,
    card_name TEXT NOT NULL UNIQUE,
    account_id INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE card_taps (
    id INTEGER PRIMARY KEY,
    tapped_on TEXT NOT NULL,
    merchant TEXT NOT NULL,
    amount INTEGER NOT NULL,
    currency TEXT NOT NULL DEFAULT 'GBP',
    card_name TEXT,
    transaction_id INTEGER UNIQUE REFERENCES transactions (id) ON DELETE SET NULL,
    dismissed INTEGER NOT NULL DEFAULT 0 CHECK (dismissed IN (0, 1)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    ai_account_id INTEGER REFERENCES accounts (id) ON DELETE SET NULL,
    ai_confidence REAL
) STRICT;

CREATE INDEX ix_card_taps_card_name ON card_taps (card_name);

-- MCP and OAuth

CREATE TABLE oauth_clients (
    id INTEGER PRIMARY KEY,
    client_id TEXT NOT NULL UNIQUE,
    client_secret_hash TEXT,
    client_name TEXT NOT NULL,
    redirect_uris TEXT NOT NULL CHECK (json_valid(redirect_uris)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
) STRICT;

CREATE TABLE oauth_codes (
    id INTEGER PRIMARY KEY,
    code_hash TEXT NOT NULL UNIQUE,
    client_id TEXT NOT NULL REFERENCES oauth_clients (client_id) ON DELETE CASCADE,
    redirect_uri TEXT NOT NULL,
    code_challenge TEXT NOT NULL,
    scopes TEXT NOT NULL CHECK (json_valid(scopes)),
    expires_at INTEGER NOT NULL,
    used INTEGER NOT NULL DEFAULT 0 CHECK (used IN (0, 1))
) STRICT;

-- Personal access tokens from Settings, and tokens issued to OAuth clients (client_id set).
CREATE TABLE access_tokens (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    token_hash TEXT NOT NULL UNIQUE,
    prefix TEXT NOT NULL,
    scopes TEXT NOT NULL CHECK (json_valid(scopes)),
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_used_at TEXT,
    client_id TEXT REFERENCES oauth_clients (client_id) ON DELETE CASCADE,
    refresh_hash TEXT UNIQUE,
    expires_at INTEGER
) STRICT;
