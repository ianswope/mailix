use rusqlite::{Connection, OptionalExtension, params};
use std::path::PathBuf;

/// A connected mail account. `provider` is `"gmail"`, `"icloud"`, or `"imap"`.
/// The IMAP/SMTP host/port fields are `None` for Gmail (which goes over the
/// REST API); `token_key` names the secret in the OS keyring (a Google refresh
/// token or an IMAP password) — see `crate::secrets`.
#[derive(Clone, Debug)]
pub struct Account {
    pub id: i64,
    pub provider: String,
    pub email: String,
    pub display_name: String,
    pub token_key: String,
    pub imap_host: Option<String>,
    pub imap_port: Option<i64>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<i64>,
    pub use_ssl: bool,
}

/// Fields for creating an account; `id` is assigned by the store.
#[derive(Clone, Debug)]
pub struct NewAccount {
    pub provider: String,
    pub email: String,
    pub display_name: String,
    pub token_key: String,
    pub imap_host: Option<String>,
    pub imap_port: Option<i64>,
    pub smtp_host: Option<String>,
    pub smtp_port: Option<i64>,
    pub use_ssl: bool,
}

/// Fields for creating/updating a conversation thread.
#[derive(Clone, Debug)]
pub struct NewThread {
    pub account_id: i64,
    pub remote_thread_id: String,
    pub subject: String,
    pub snippet: String,
    pub sender: String,
    pub last_date: String,
    pub unread: bool,
    pub starred: bool,
    pub has_attachments: bool,
}

/// Fields for creating/updating a message. `has_body` is intentionally absent:
/// it's set only when a body is cached (see `set_body`), so re-syncing metadata
/// never clobbers a fetched body.
#[derive(Clone, Debug)]
pub struct NewMessage {
    pub account_id: i64,
    pub thread_id: i64,
    pub remote_msg_id: String,
    pub from_addr: Option<String>,
    pub to_addrs: Option<String>,
    pub cc_addrs: Option<String>,
    pub date: Option<String>,
    pub subject: Option<String>,
    pub snippet: Option<String>,
    pub seen: bool,
}

/// A conversation row for the thread list.
#[derive(Clone, Debug)]
pub struct ThreadRow {
    pub id: i64,
    pub account_id: i64,
    pub remote_thread_id: String,
    pub subject: String,
    pub snippet: String,
    pub sender: String,
    pub last_date: String,
    pub unread: bool,
    pub starred: bool,
}

/// A message row within an opened thread.
#[derive(Clone, Debug)]
pub struct MessageRow {
    pub id: i64,
    pub account_id: i64,
    pub remote_msg_id: String,
    pub from_addr: Option<String>,
    pub to_addrs: Option<String>,
    pub date: Option<String>,
    pub subject: Option<String>,
    pub snippet: Option<String>,
    pub seen: bool,
    pub has_body: bool,
}

/// A send-as identity: an address the user can send from, with its native
/// Gmail signature. `account_id` ties it to the account whose token sends it.
#[derive(Clone, Debug)]
pub struct Identity {
    pub account_id: i64,
    pub email: String,
    pub display_name: String,
    pub signature: String,
    pub is_default: bool,
}

/// A message's cached body in its available representations.
#[derive(Clone, Debug, Default)]
pub struct Body {
    pub text_plain: Option<String>,
    pub html: Option<String>,
    pub sanitized_html: Option<String>,
}

/// SQLite-backed local cache. Each thread (the GTK main thread and every
/// background sync worker) opens its own `Store`; `rusqlite::Connection` isn't
/// `Send`, so they are never shared. WAL + a busy timeout lets the connections
/// overlap instead of failing immediately with `database is locked` — the same
/// concurrency model calix uses.
pub struct Store {
    conn: Connection,
}

const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS accounts (
        id INTEGER PRIMARY KEY,
        provider TEXT NOT NULL,
        email TEXT NOT NULL,
        display_name TEXT NOT NULL,
        token_key TEXT NOT NULL,
        imap_host TEXT,
        imap_port INTEGER,
        smtp_host TEXT,
        smtp_port INTEGER,
        use_ssl INTEGER NOT NULL DEFAULT 1,
        UNIQUE(provider, email)
    );
    -- Gmail labels; IMAP folders map onto the same table (kind='folder').
    CREATE TABLE IF NOT EXISTS labels (
        id INTEGER PRIMARY KEY,
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        remote_id TEXT NOT NULL,
        name TEXT NOT NULL,
        kind TEXT NOT NULL DEFAULT 'user',
        color TEXT,
        unread_count INTEGER NOT NULL DEFAULT 0,
        UNIQUE(account_id, remote_id)
    );
    CREATE TABLE IF NOT EXISTS threads (
        id INTEGER PRIMARY KEY,
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        remote_thread_id TEXT NOT NULL,
        subject TEXT,
        snippet TEXT,
        sender TEXT,
        last_date TEXT,
        unread INTEGER NOT NULL DEFAULT 0,
        starred INTEGER NOT NULL DEFAULT 0,
        has_attachments INTEGER NOT NULL DEFAULT 0,
        UNIQUE(account_id, remote_thread_id)
    );
    CREATE TABLE IF NOT EXISTS messages (
        id INTEGER PRIMARY KEY,
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        thread_id INTEGER REFERENCES threads(id) ON DELETE CASCADE,
        remote_msg_id TEXT NOT NULL,
        imap_uid INTEGER,
        from_addr TEXT,
        to_addrs TEXT,
        cc_addrs TEXT,
        date TEXT,
        subject TEXT,
        snippet TEXT,
        seen INTEGER NOT NULL DEFAULT 0,
        flagged INTEGER NOT NULL DEFAULT 0,
        draft INTEGER NOT NULL DEFAULT 0,
        answered INTEGER NOT NULL DEFAULT 0,
        has_body INTEGER NOT NULL DEFAULT 0,
        UNIQUE(account_id, remote_msg_id)
    );
    -- Gmail's many-to-many: a message can carry several labels at once.
    CREATE TABLE IF NOT EXISTS message_labels (
        message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
        label_id INTEGER NOT NULL REFERENCES labels(id) ON DELETE CASCADE,
        PRIMARY KEY (message_id, label_id)
    );
    -- Bodies are fetched on demand and cached here, enabling offline re-read.
    CREATE TABLE IF NOT EXISTS bodies (
        message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
        text_plain TEXT,
        html TEXT,
        sanitized_html TEXT,
        loaded_at TEXT
    );
    CREATE TABLE IF NOT EXISTS attachments (
        id INTEGER PRIMARY KEY,
        message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
        filename TEXT,
        mime TEXT,
        size INTEGER,
        cid TEXT,
        part_id TEXT,
        cached_path TEXT
    );
    CREATE TABLE IF NOT EXISTS sync_state (
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        scope TEXT NOT NULL,
        history_id TEXT,
        uidvalidity INTEGER,
        uidnext INTEGER,
        highestmodseq INTEGER,
        PRIMARY KEY (account_id, scope)
    );
    -- Send-as identities (with native Gmail signatures), refreshed each sync.
    CREATE TABLE IF NOT EXISTS identities (
        id INTEGER PRIMARY KEY,
        account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
        email TEXT NOT NULL,
        display_name TEXT,
        signature TEXT,
        is_default INTEGER NOT NULL DEFAULT 0,
        UNIQUE(account_id, email)
    );
    CREATE TABLE IF NOT EXISTS app_settings (
        key TEXT PRIMARY KEY,
        value TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS messages_thread ON messages(thread_id);
    CREATE INDEX IF NOT EXISTS threads_account_date ON threads(account_id, last_date);
";

impl Store {
    pub fn open() -> rusqlite::Result<Self> {
        let path = data_file_path();
        std::fs::create_dir_all(path.parent().expect("data file has a parent dir"))
            .expect("can create Mailix data directory");
        Self::from_connection(Connection::open(path)?)
    }

    #[cfg(test)]
    fn open_in_memory() -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open_in_memory()?)
    }

    fn from_connection(conn: Connection) -> rusqlite::Result<Self> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(SCHEMA)?;
        // Lightweight forward migrations for stores created by an earlier build.
        ensure_column(&conn, "threads", "sender", "TEXT")?;
        ensure_column(&conn, "threads", "starred", "INTEGER NOT NULL DEFAULT 0")?;
        Ok(Self { conn })
    }

    // --- app_settings ---

    pub fn setting(&self, key: &str) -> rusqlite::Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT value FROM app_settings WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()
    }

    pub fn set_setting(&self, key: &str, value: &str) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO app_settings (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // --- accounts ---

    pub fn insert_account(&self, account: &NewAccount) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO accounts
                (provider, email, display_name, token_key,
                 imap_host, imap_port, smtp_host, smtp_port, use_ssl)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(provider, email) DO UPDATE SET
                display_name = excluded.display_name,
                token_key    = excluded.token_key,
                imap_host    = excluded.imap_host,
                imap_port    = excluded.imap_port,
                smtp_host    = excluded.smtp_host,
                smtp_port    = excluded.smtp_port,
                use_ssl      = excluded.use_ssl",
            params![
                account.provider,
                account.email,
                account.display_name,
                account.token_key,
                account.imap_host,
                account.imap_port,
                account.smtp_host,
                account.smtp_port,
                account.use_ssl as i64,
            ],
        )?;
        // ON CONFLICT updates don't change last_insert_rowid reliably, so look
        // the id up by its natural key.
        self.conn.query_row(
            "SELECT id FROM accounts WHERE provider = ?1 AND email = ?2",
            params![account.provider, account.email],
            |row| row.get(0),
        )
    }

    pub fn list_accounts(&self) -> rusqlite::Result<Vec<Account>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, provider, email, display_name, token_key,
                    imap_host, imap_port, smtp_host, smtp_port, use_ssl
             FROM accounts ORDER BY id",
        )?;
        let rows = stmt.query_map([], Self::row_to_account)?;
        rows.collect()
    }

    pub fn account(&self, id: i64) -> rusqlite::Result<Option<Account>> {
        self.conn
            .query_row(
                "SELECT id, provider, email, display_name, token_key,
                        imap_host, imap_port, smtp_host, smtp_port, use_ssl
                 FROM accounts WHERE id = ?1",
                params![id],
                Self::row_to_account,
            )
            .optional()
    }

    pub fn delete_account(&self, id: i64) -> rusqlite::Result<()> {
        // ON DELETE CASCADE clears the account's labels/threads/messages/etc.
        self.conn
            .execute("DELETE FROM accounts WHERE id = ?1", params![id])?;
        Ok(())
    }

    // --- labels ---

    pub fn upsert_label(
        &self,
        account_id: i64,
        remote_id: &str,
        name: &str,
        kind: &str,
        color: Option<&str>,
    ) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO labels (account_id, remote_id, name, kind, color)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(account_id, remote_id) DO UPDATE SET
                name = excluded.name, kind = excluded.kind, color = excluded.color",
            params![account_id, remote_id, name, kind, color],
        )?;
        self.conn.query_row(
            "SELECT id FROM labels WHERE account_id = ?1 AND remote_id = ?2",
            params![account_id, remote_id],
            |row| row.get(0),
        )
    }

    // --- threads & messages ---

    pub fn upsert_thread(&self, thread: &NewThread) -> rusqlite::Result<i64> {
        self.conn.execute(
            "INSERT INTO threads
                (account_id, remote_thread_id, subject, snippet, sender, last_date,
                 unread, starred, has_attachments)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(account_id, remote_thread_id) DO UPDATE SET
                subject = excluded.subject, snippet = excluded.snippet,
                sender = excluded.sender, last_date = excluded.last_date,
                unread = excluded.unread, starred = excluded.starred,
                has_attachments = excluded.has_attachments",
            params![
                thread.account_id,
                thread.remote_thread_id,
                thread.subject,
                thread.snippet,
                thread.sender,
                thread.last_date,
                thread.unread as i64,
                thread.starred as i64,
                thread.has_attachments as i64,
            ],
        )?;
        self.conn.query_row(
            "SELECT id FROM threads WHERE account_id = ?1 AND remote_thread_id = ?2",
            params![thread.account_id, thread.remote_thread_id],
            |row| row.get(0),
        )
    }

    pub fn upsert_message(&self, message: &NewMessage) -> rusqlite::Result<i64> {
        // `has_body` is deliberately not touched here — see `NewMessage`.
        self.conn.execute(
            "INSERT INTO messages
                (account_id, thread_id, remote_msg_id, from_addr, to_addrs, cc_addrs,
                 date, subject, snippet, seen)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
             ON CONFLICT(account_id, remote_msg_id) DO UPDATE SET
                thread_id = excluded.thread_id, from_addr = excluded.from_addr,
                to_addrs = excluded.to_addrs, cc_addrs = excluded.cc_addrs,
                date = excluded.date, subject = excluded.subject,
                snippet = excluded.snippet, seen = excluded.seen",
            params![
                message.account_id,
                message.thread_id,
                message.remote_msg_id,
                message.from_addr,
                message.to_addrs,
                message.cc_addrs,
                message.date,
                message.subject,
                message.snippet,
                message.seen as i64,
            ],
        )?;
        self.conn.query_row(
            "SELECT id FROM messages WHERE account_id = ?1 AND remote_msg_id = ?2",
            params![message.account_id, message.remote_msg_id],
            |row| row.get(0),
        )
    }

    pub fn set_message_labels(&self, message_id: i64, label_ids: &[i64]) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM message_labels WHERE message_id = ?1",
            params![message_id],
        )?;
        for label_id in label_ids {
            self.conn.execute(
                "INSERT OR IGNORE INTO message_labels (message_id, label_id) VALUES (?1, ?2)",
                params![message_id, label_id],
            )?;
        }
        Ok(())
    }

    pub fn set_body(
        &self,
        message_id: i64,
        text_plain: Option<&str>,
        html: Option<&str>,
        sanitized_html: Option<&str>,
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "INSERT INTO bodies (message_id, text_plain, html, sanitized_html, loaded_at)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(message_id) DO UPDATE SET
                text_plain = excluded.text_plain, html = excluded.html,
                sanitized_html = excluded.sanitized_html, loaded_at = excluded.loaded_at",
            params![message_id, text_plain, html, sanitized_html, now_iso()],
        )?;
        self.conn
            .execute("UPDATE messages SET has_body = 1 WHERE id = ?1", params![message_id])?;
        Ok(())
    }

    pub fn get_body(&self, message_id: i64) -> rusqlite::Result<Option<Body>> {
        self.conn
            .query_row(
                "SELECT text_plain, html, sanitized_html FROM bodies WHERE message_id = ?1",
                params![message_id],
                |row| {
                    Ok(Body {
                        text_plain: row.get(0)?,
                        html: row.get(1)?,
                        sanitized_html: row.get(2)?,
                    })
                },
            )
            .optional()
    }

    /// Conversations for the (single, combined) inbox view, newest first.
    pub fn list_inbox_threads(&self) -> rusqlite::Result<Vec<ThreadRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, account_id, remote_thread_id, subject, snippet, sender, last_date,
                    unread, starred
             FROM threads ORDER BY last_date DESC",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(ThreadRow {
                id: row.get(0)?,
                account_id: row.get(1)?,
                remote_thread_id: row.get(2)?,
                subject: row.get(3)?,
                snippet: row.get(4)?,
                sender: row.get::<_, Option<String>>(5)?.unwrap_or_default(),
                last_date: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
                unread: row.get::<_, i64>(7)? != 0,
                starred: row.get::<_, i64>(8)? != 0,
            })
        })?;
        rows.collect()
    }

    /// Messages within an opened thread, oldest first.
    pub fn messages_in_thread(&self, thread_id: i64) -> rusqlite::Result<Vec<MessageRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, account_id, remote_msg_id, from_addr, to_addrs, date, subject,
                    snippet, seen, has_body
             FROM messages WHERE thread_id = ?1 ORDER BY date ASC",
        )?;
        let rows = stmt.query_map(params![thread_id], |row| {
            Ok(MessageRow {
                id: row.get(0)?,
                account_id: row.get(1)?,
                remote_msg_id: row.get(2)?,
                from_addr: row.get(3)?,
                to_addrs: row.get(4)?,
                date: row.get(5)?,
                subject: row.get(6)?,
                snippet: row.get(7)?,
                seen: row.get::<_, i64>(8)? != 0,
                has_body: row.get::<_, i64>(9)? != 0,
            })
        })?;
        rows.collect()
    }

    pub fn set_thread_unread(&self, thread_id: i64, unread: bool) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE threads SET unread = ?2 WHERE id = ?1",
            params![thread_id, unread as i64],
        )?;
        // Keep the per-message flag consistent so a later render/list agrees.
        self.conn.execute(
            "UPDATE messages SET seen = ?2 WHERE thread_id = ?1",
            params![thread_id, (!unread) as i64],
        )?;
        Ok(())
    }

    pub fn set_thread_starred(&self, thread_id: i64, starred: bool) -> rusqlite::Result<()> {
        self.conn.execute(
            "UPDATE threads SET starred = ?2 WHERE id = ?1",
            params![thread_id, starred as i64],
        )?;
        Ok(())
    }

    /// Removes a thread (and, by cascade, its messages/bodies) from the local
    /// cache — used after archiving or trashing, since neither reappears in a
    /// future `in:inbox` sync.
    pub fn delete_thread(&self, thread_id: i64) -> rusqlite::Result<()> {
        self.conn
            .execute("DELETE FROM threads WHERE id = ?1", params![thread_id])?;
        Ok(())
    }

    // --- send identities ---

    /// Replaces an account's identities wholesale (called each sync).
    pub fn replace_identities(
        &self,
        account_id: i64,
        identities: &[Identity],
    ) -> rusqlite::Result<()> {
        self.conn.execute(
            "DELETE FROM identities WHERE account_id = ?1",
            params![account_id],
        )?;
        for identity in identities {
            self.conn.execute(
                "INSERT INTO identities (account_id, email, display_name, signature, is_default)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    account_id,
                    identity.email,
                    identity.display_name,
                    identity.signature,
                    identity.is_default as i64,
                ],
            )?;
        }
        Ok(())
    }

    /// All identities across all accounts (defaults first) for the composer's
    /// From picker.
    pub fn list_identities(&self) -> rusqlite::Result<Vec<Identity>> {
        let mut stmt = self.conn.prepare(
            "SELECT account_id, email, display_name, signature, is_default
             FROM identities ORDER BY is_default DESC, email",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(Identity {
                account_id: row.get(0)?,
                email: row.get(1)?,
                display_name: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                signature: row.get::<_, Option<String>>(3)?.unwrap_or_default(),
                is_default: row.get::<_, i64>(4)? != 0,
            })
        })?;
        rows.collect()
    }

    fn row_to_account(row: &rusqlite::Row<'_>) -> rusqlite::Result<Account> {
        Ok(Account {
            id: row.get(0)?,
            provider: row.get(1)?,
            email: row.get(2)?,
            display_name: row.get(3)?,
            token_key: row.get(4)?,
            imap_host: row.get(5)?,
            imap_port: row.get(6)?,
            smtp_host: row.get(7)?,
            smtp_port: row.get(8)?,
            use_ssl: row.get::<_, i64>(9)? != 0,
        })
    }
}

fn data_file_path() -> PathBuf {
    gtk::glib::user_data_dir()
        .join("mailix")
        .join("mailix.sqlite3")
}

fn now_iso() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// Adds `column` to `table` if a store from an earlier build predates it. Table
/// and column names are internal constants, so interpolating them is safe.
fn ensure_column(conn: &Connection, table: &str, column: &str, decl: &str) -> rusqlite::Result<()> {
    let count: i64 = conn.query_row(
        &format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = '{column}'"),
        [],
        |row| row.get(0),
    )?;
    if count == 0 {
        conn.execute(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl}"), [])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gmail(email: &str) -> NewAccount {
        NewAccount {
            provider: "gmail".into(),
            email: email.into(),
            display_name: email.into(),
            token_key: crate::secrets::google_refresh_key(email),
            imap_host: None,
            imap_port: None,
            smtp_host: None,
            smtp_port: None,
            use_ssl: true,
        }
    }

    #[test]
    fn settings_round_trip() {
        let store = Store::open_in_memory().unwrap();
        assert_eq!(store.setting("unified_inbox").unwrap(), None);
        store.set_setting("unified_inbox", "true").unwrap();
        store.set_setting("unified_inbox", "false").unwrap();
        assert_eq!(
            store.setting("unified_inbox").unwrap(),
            Some("false".into())
        );
    }

    #[test]
    fn account_insert_is_idempotent_on_provider_email() {
        let store = Store::open_in_memory().unwrap();
        let id1 = store.insert_account(&gmail("me@example.com")).unwrap();
        let id2 = store.insert_account(&gmail("me@example.com")).unwrap();
        assert_eq!(id1, id2, "re-adding the same account reuses its row");
        assert_eq!(store.list_accounts().unwrap().len(), 1);
    }

    #[test]
    fn account_round_trip_and_delete() {
        let store = Store::open_in_memory().unwrap();
        let id = store.insert_account(&gmail("a@example.com")).unwrap();
        store.insert_account(&gmail("b@example.com")).unwrap();

        let fetched = store.account(id).unwrap().unwrap();
        assert_eq!(fetched.provider, "gmail");
        assert_eq!(fetched.email, "a@example.com");
        assert!(fetched.use_ssl);
        assert_eq!(store.list_accounts().unwrap().len(), 2);

        store.delete_account(id).unwrap();
        assert!(store.account(id).unwrap().is_none());
        assert_eq!(store.list_accounts().unwrap().len(), 1);
    }

    fn thread(store: &Store, account_id: i64, remote: &str, unread: bool) -> i64 {
        store
            .upsert_thread(&NewThread {
                account_id,
                remote_thread_id: remote.into(),
                subject: "Subject".into(),
                snippet: "snippet".into(),
                sender: "Sender <s@example.com>".into(),
                last_date: "2026-07-15T00:00:00+00:00".into(),
                unread,
                starred: false,
                has_attachments: false,
            })
            .unwrap()
    }

    #[test]
    fn thread_read_and_star_toggle() {
        let store = Store::open_in_memory().unwrap();
        let account_id = store.insert_account(&gmail("a@example.com")).unwrap();
        let tid = thread(&store, account_id, "t1", true);

        let row = &store.list_inbox_threads().unwrap()[0];
        assert!(row.unread && !row.starred);

        store.set_thread_unread(tid, false).unwrap();
        store.set_thread_starred(tid, true).unwrap();
        let row = &store.list_inbox_threads().unwrap()[0];
        assert!(!row.unread && row.starred);
    }

    #[test]
    fn delete_thread_cascades_to_messages_and_bodies() {
        let store = Store::open_in_memory().unwrap();
        let account_id = store.insert_account(&gmail("a@example.com")).unwrap();
        let tid = thread(&store, account_id, "t1", false);
        let mid = store
            .upsert_message(&NewMessage {
                account_id,
                thread_id: tid,
                remote_msg_id: "m1".into(),
                from_addr: Some("s@example.com".into()),
                to_addrs: None,
                cc_addrs: None,
                date: Some("2026-07-15T00:00:00+00:00".into()),
                subject: Some("Subject".into()),
                snippet: None,
                seen: true,
            })
            .unwrap();
        store
            .set_body(mid, Some("hello"), None, Some("<p>hello</p>"))
            .unwrap();
        assert!(store.get_body(mid).unwrap().is_some());
        assert_eq!(store.messages_in_thread(tid).unwrap().len(), 1);

        store.delete_thread(tid).unwrap();
        assert!(store.list_inbox_threads().unwrap().is_empty());
        assert!(store.messages_in_thread(tid).unwrap().is_empty());
        // ON DELETE CASCADE chains threads -> messages -> bodies.
        assert!(store.get_body(mid).unwrap().is_none());
    }
}
