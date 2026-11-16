-- Tags: labels on transactions, as many as the user likes, such as who paid or which trip it
-- was for. A tag is kept while a transaction has it.

CREATE TABLE tags (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL UNIQUE COLLATE NOCASE
) STRICT;

CREATE TABLE transaction_tags (
    transaction_id INTEGER NOT NULL REFERENCES transactions (id) ON DELETE CASCADE,
    tag_id INTEGER NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
    PRIMARY KEY (transaction_id, tag_id)
) STRICT;

CREATE INDEX ix_transaction_tags_tag_id ON transaction_tags (tag_id);
