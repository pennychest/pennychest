# Backing up and restoring

Everything PennyChest knows is in one SQLite database: accounts, transactions, rules, budgets,
settings and the AI logs. Uploaded statements are kept next to it, in the upload directory, so
that each import's original file can be downloaded again.

## Taking a backup

In **Settings > Export**, **Download database** saves a copy of the whole database as
`pennychest-backup-<date>.db`. It's safe to take while PennyChest is in use: the copy is made in
one read, so it's consistent even if something changes during it.

The same download is at `GET /api/export/database` for a signed-in session, so a script can
take backups on a schedule.

Keep backups somewhere safe. They hold your full financial history, and the hashes of your
password and access tokens.

## Restoring one

A backup is a complete PennyChest database. To restore it:

1. Stop PennyChest.
2. In the data directory (`/data` in the Docker image, or wherever
   `PENNYCHEST_DATABASE_PATH` points), move the current database aside, along with its
   `-wal` and `-shm` files if they're there:

   ```sh
   mv pennychest.db pennychest.db.old
   mv pennychest.db-wal pennychest.db-wal.old 2>/dev/null
   mv pennychest.db-shm pennychest.db-shm.old 2>/dev/null
   ```

3. Copy the backup in under the database's name:

   ```sh
   cp pennychest-backup-2026-10-14.db pennychest.db
   ```

4. Start PennyChest. A backup from an older version is brought up to date as it starts.

Sign in with the password you had when the backup was taken. Access tokens and connected apps
are as they were then too, so ones made since need making again.

Uploaded statement files aren't in the backup. Imports from it still work without them; only
**Download original file** on those imports needs them. Copy the upload directory too if you
want them back.
