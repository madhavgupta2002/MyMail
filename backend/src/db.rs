//! SQLite storage layer: schema, models and queries.

use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use rusqlite::{Connection, OptionalExtension, Row, ToSql, params, params_from_iter};
use serde::{Deserialize, Serialize};

pub const FOLDERS: &[&str] = &["inbox", "starred", "sent", "drafts", "archive", "trash"];
/// Folders a message can physically live in ("starred" is a virtual view).
const STORAGE_FOLDERS: &[&str] = &["inbox", "sent", "drafts", "archive", "trash"];

#[derive(Debug)]
pub enum DbError {
    NotFound(&'static str),
    Invalid(String),
    Conflict(String),
    Sql(rusqlite::Error),
}

impl std::error::Error for DbError {}

impl std::fmt::Display for DbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DbError::NotFound(what) => write!(f, "{what} not found"),
            DbError::Invalid(msg) | DbError::Conflict(msg) => f.write_str(msg),
            DbError::Sql(e) => write!(f, "database error: {e}"),
        }
    }
}

impl From<rusqlite::Error> for DbError {
    fn from(e: rusqlite::Error) -> Self {
        DbError::Sql(e)
    }
}

pub type DbResult<T> = Result<T, DbError>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Account {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Message {
    pub id: i64,
    pub account_id: i64,
    pub folder: String,
    pub from_name: String,
    pub from_addr: String,
    pub to_addrs: String,
    pub cc_addrs: String,
    pub subject: String,
    pub body: String,
    pub is_read: bool,
    pub is_starred: bool,
    pub in_reply_to: Option<i64>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MessageSummary {
    pub id: i64,
    pub account_id: i64,
    pub folder: String,
    pub from_name: String,
    pub from_addr: String,
    pub to_addrs: String,
    pub subject: String,
    pub snippet: String,
    pub is_read: bool,
    pub is_starred: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderCount {
    pub folder: String,
    pub total: i64,
    pub unread: i64,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Compose {
    pub account_id: i64,
    #[serde(default)]
    pub to: String,
    #[serde(default)]
    pub cc: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub in_reply_to: Option<i64>,
    /// When sending a saved draft, the draft is removed after delivery.
    #[serde(default)]
    pub draft_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SendResult {
    pub message: Message,
    /// Number of local mailboxes the message was delivered to.
    pub delivered_local: usize,
    /// Recipients that are not local accounts (not delivered; no outbound relay).
    pub undelivered: Vec<String>,
}

/// Incoming message (e.g. received over SMTP) to deliver to local inboxes.
#[derive(Debug, Clone, Default)]
pub struct Incoming {
    pub from_name: String,
    pub from_addr: String,
    pub to_addrs: String,
    pub cc_addrs: String,
    pub subject: String,
    pub body: String,
    pub created_at: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct MessagePatch {
    pub is_read: Option<bool>,
    pub is_starred: Option<bool>,
    pub folder: Option<String>,
}

#[derive(Clone)]
pub struct Db {
    conn: Arc<Mutex<Connection>>,
}

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS accounts (
    id          INTEGER PRIMARY KEY,
    name        TEXT NOT NULL,
    email       TEXT NOT NULL UNIQUE COLLATE NOCASE,
    created_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    id           INTEGER PRIMARY KEY,
    account_id   INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    folder       TEXT NOT NULL,
    from_name    TEXT NOT NULL,
    from_addr    TEXT NOT NULL,
    to_addrs     TEXT NOT NULL,
    cc_addrs     TEXT NOT NULL DEFAULT '',
    subject      TEXT NOT NULL,
    body         TEXT NOT NULL,
    is_read      INTEGER NOT NULL DEFAULT 0,
    is_starred   INTEGER NOT NULL DEFAULT 0,
    in_reply_to  INTEGER,
    created_at   INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_messages_account_folder
    ON messages(account_id, folder, created_at DESC);

CREATE VIRTUAL TABLE IF NOT EXISTS messages_fts USING fts5(
    subject, body, from_name, from_addr, to_addrs,
    content='messages', content_rowid='id'
);

CREATE TRIGGER IF NOT EXISTS messages_ai AFTER INSERT ON messages BEGIN
    INSERT INTO messages_fts(rowid, subject, body, from_name, from_addr, to_addrs)
    VALUES (new.id, new.subject, new.body, new.from_name, new.from_addr, new.to_addrs);
END;

CREATE TRIGGER IF NOT EXISTS messages_ad AFTER DELETE ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, subject, body, from_name, from_addr, to_addrs)
    VALUES ('delete', old.id, old.subject, old.body, old.from_name, old.from_addr, old.to_addrs);
END;

CREATE TRIGGER IF NOT EXISTS messages_au AFTER UPDATE OF subject, body, from_name, from_addr, to_addrs ON messages BEGIN
    INSERT INTO messages_fts(messages_fts, rowid, subject, body, from_name, from_addr, to_addrs)
    VALUES ('delete', old.id, old.subject, old.body, old.from_name, old.from_addr, old.to_addrs);
    INSERT INTO messages_fts(rowid, subject, body, from_name, from_addr, to_addrs)
    VALUES (new.id, new.subject, new.body, new.from_name, new.from_addr, new.to_addrs);
END;
"#;

const MESSAGE_COLUMNS: &str = "id, account_id, folder, from_name, from_addr, to_addrs, cc_addrs, \
     subject, body, is_read, is_starred, in_reply_to, created_at";

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

fn row_to_message(row: &Row<'_>) -> rusqlite::Result<Message> {
    Ok(Message {
        id: row.get(0)?,
        account_id: row.get(1)?,
        folder: row.get(2)?,
        from_name: row.get(3)?,
        from_addr: row.get(4)?,
        to_addrs: row.get(5)?,
        cc_addrs: row.get(6)?,
        subject: row.get(7)?,
        body: row.get(8)?,
        is_read: row.get(9)?,
        is_starred: row.get(10)?,
        in_reply_to: row.get(11)?,
        created_at: row.get(12)?,
    })
}

fn row_to_account(row: &Row<'_>) -> rusqlite::Result<Account> {
    Ok(Account {
        id: row.get(0)?,
        name: row.get(1)?,
        email: row.get(2)?,
        created_at: row.get(3)?,
    })
}

/// Parses a comma/semicolon separated recipient list ("Name <a@b>, c@d") into
/// lowercase bare addresses. Returns an error for anything that isn't an address.
pub fn parse_addresses(input: &str) -> DbResult<Vec<String>> {
    let mut out = Vec::new();
    for part in input.split([',', ';']) {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let addr = match (part.rfind('<'), part.rfind('>')) {
            (Some(start), Some(end)) if start < end => &part[start + 1..end],
            _ => part,
        }
        .trim()
        .to_lowercase();
        if !is_valid_address(&addr) {
            return Err(DbError::Invalid(format!("invalid email address: {part}")));
        }
        out.push(addr);
    }
    Ok(out)
}

pub fn is_valid_address(addr: &str) -> bool {
    let Some((local, domain)) = addr.split_once('@') else {
        return false;
    };
    !local.is_empty()
        && !domain.is_empty()
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !addr.contains(char::is_whitespace)
        && !domain.contains('@')
}

/// Builds a safe FTS5 query: each whitespace-separated term becomes a quoted
/// prefix match, so user input can't inject FTS syntax.
fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split_whitespace()
        .map(|t| format!("\"{}\"*", t.replace('"', "\"\"")))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" "))
}

fn snippet(body: &str) -> String {
    let collapsed = body.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut s: String = collapsed.chars().take(140).collect();
    if collapsed.chars().count() > 140 {
        s.push('…');
    }
    s
}

impl Db {
    pub fn open(path: &Path) -> DbResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DbError::Invalid(format!("cannot create data dir: {e}")))?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> DbResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> DbResult<Self> {
        conn.execute_batch(SCHEMA)?;
        Ok(Db {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    fn conn(&self) -> MutexGuard<'_, Connection> {
        // A panic while holding the lock doesn't corrupt SQLite state; recover.
        self.conn.lock().unwrap_or_else(|p| p.into_inner())
    }

    // ---------- accounts ----------

    pub fn list_accounts(&self) -> DbResult<Vec<Account>> {
        let conn = self.conn();
        let mut stmt =
            conn.prepare("SELECT id, name, email, created_at FROM accounts ORDER BY id")?;
        let rows = stmt.query_map([], row_to_account)?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get_account(&self, id: i64) -> DbResult<Account> {
        self.conn()
            .query_row(
                "SELECT id, name, email, created_at FROM accounts WHERE id = ?1",
                [id],
                row_to_account,
            )
            .optional()?
            .ok_or(DbError::NotFound("account"))
    }

    pub fn find_account_by_email(&self, email: &str) -> DbResult<Option<Account>> {
        Ok(self
            .conn()
            .query_row(
                "SELECT id, name, email, created_at FROM accounts WHERE email = ?1",
                [email.trim()],
                row_to_account,
            )
            .optional()?)
    }

    pub fn create_account(&self, name: &str, email: &str) -> DbResult<Account> {
        let name = name.trim();
        let email = email.trim().to_lowercase();
        if name.is_empty() {
            return Err(DbError::Invalid("name is required".into()));
        }
        if !is_valid_address(&email) {
            return Err(DbError::Invalid(format!("invalid email address: {email}")));
        }
        if self.find_account_by_email(&email)?.is_some() {
            return Err(DbError::Conflict(format!("account {email} already exists")));
        }
        let created_at = now();
        let id = {
            let conn = self.conn();
            conn.execute(
                "INSERT INTO accounts (name, email, created_at) VALUES (?1, ?2, ?3)",
                params![name, email, created_at],
            )?;
            conn.last_insert_rowid()
        };
        Ok(Account {
            id,
            name: name.to_string(),
            email,
            created_at,
        })
    }

    pub fn delete_account(&self, id: i64) -> DbResult<()> {
        let n = self
            .conn()
            .execute("DELETE FROM accounts WHERE id = ?1", [id])?;
        if n == 0 {
            return Err(DbError::NotFound("account"));
        }
        Ok(())
    }

    // ---------- messages ----------

    pub fn folder_counts(&self, account_id: i64) -> DbResult<Vec<FolderCount>> {
        self.get_account(account_id)?;
        let conn = self.conn();
        let mut stmt = conn.prepare(
            "SELECT folder, COUNT(*), COALESCE(SUM(is_read = 0), 0) FROM messages \
             WHERE account_id = ?1 GROUP BY folder \
             UNION ALL \
             SELECT 'starred', COUNT(*), COALESCE(SUM(is_read = 0), 0) FROM messages \
             WHERE account_id = ?1 AND is_starred = 1 AND folder != 'trash'",
        )?;
        let rows: Vec<(String, i64, i64)> = stmt
            .query_map([account_id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<Result<_, _>>()?;
        Ok(FOLDERS
            .iter()
            .map(|folder| {
                let (total, unread) = rows
                    .iter()
                    .find(|(f, _, _)| f == folder)
                    .map_or((0, 0), |(_, t, u)| (*t, *u));
                FolderCount {
                    folder: folder.to_string(),
                    total,
                    unread,
                }
            })
            .collect())
    }

    pub fn list_messages(
        &self,
        account_id: i64,
        folder: &str,
        query: Option<&str>,
    ) -> DbResult<Vec<MessageSummary>> {
        if !FOLDERS.contains(&folder) && folder != "all" {
            return Err(DbError::Invalid(format!("unknown folder: {folder}")));
        }
        let mut sql = String::from(
            "SELECT m.id, m.account_id, m.folder, m.from_name, m.from_addr, m.to_addrs, \
                    m.subject, substr(m.body, 1, 400), m.is_read, m.is_starred, m.created_at \
             FROM messages m WHERE m.account_id = ?",
        );
        let mut args: Vec<Box<dyn ToSql>> = vec![Box::new(account_id)];
        match folder {
            "starred" => sql.push_str(" AND m.is_starred = 1 AND m.folder != 'trash'"),
            "all" => sql.push_str(" AND m.folder != 'trash'"),
            _ => {
                sql.push_str(" AND m.folder = ?");
                args.push(Box::new(folder.to_string()));
            }
        }
        if let Some(fts) = query.and_then(fts_query) {
            sql.push_str(
                " AND m.id IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ?)",
            );
            args.push(Box::new(fts));
        }
        sql.push_str(" ORDER BY m.created_at DESC, m.id DESC LIMIT 500");

        let conn = self.conn();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args.iter()), |r| {
            Ok(MessageSummary {
                id: r.get(0)?,
                account_id: r.get(1)?,
                folder: r.get(2)?,
                from_name: r.get(3)?,
                from_addr: r.get(4)?,
                to_addrs: r.get(5)?,
                subject: r.get(6)?,
                snippet: snippet(&r.get::<_, String>(7)?),
                is_read: r.get(8)?,
                is_starred: r.get(9)?,
                created_at: r.get(10)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    pub fn get_message(&self, id: i64) -> DbResult<Message> {
        self.conn()
            .query_row(
                &format!("SELECT {MESSAGE_COLUMNS} FROM messages WHERE id = ?1"),
                [id],
                row_to_message,
            )
            .optional()?
            .ok_or(DbError::NotFound("message"))
    }

    pub fn update_message(&self, id: i64, patch: &MessagePatch) -> DbResult<Message> {
        let current = self.get_message(id)?;
        if let Some(folder) = &patch.folder {
            if !STORAGE_FOLDERS.contains(&folder.as_str()) {
                return Err(DbError::Invalid(format!("cannot move to folder: {folder}")));
            }
            if (folder == "drafts") != (current.folder == "drafts") {
                return Err(DbError::Invalid("drafts cannot be moved in or out".into()));
            }
        }
        self.conn().execute(
            "UPDATE messages SET is_read = COALESCE(?2, is_read), \
                 is_starred = COALESCE(?3, is_starred), folder = COALESCE(?4, folder) \
             WHERE id = ?1",
            params![id, patch.is_read, patch.is_starred, patch.folder],
        )?;
        self.get_message(id)
    }

    /// Moves a message to trash, or permanently deletes it if it is already in
    /// trash or is a draft. Returns true when permanently deleted.
    pub fn delete_message(&self, id: i64) -> DbResult<bool> {
        let msg = self.get_message(id)?;
        if msg.folder == "trash" || msg.folder == "drafts" {
            self.conn()
                .execute("DELETE FROM messages WHERE id = ?1", [id])?;
            Ok(true)
        } else {
            self.conn()
                .execute("UPDATE messages SET folder = 'trash' WHERE id = ?1", [id])?;
            Ok(false)
        }
    }

    pub fn empty_trash(&self, account_id: i64) -> DbResult<usize> {
        self.get_account(account_id)?;
        Ok(self.conn().execute(
            "DELETE FROM messages WHERE account_id = ?1 AND folder = 'trash'",
            [account_id],
        )?)
    }

    pub fn mark_all_read(&self, account_id: i64, folder: &str) -> DbResult<usize> {
        self.get_account(account_id)?;
        let conn = self.conn();
        let n = match folder {
            "starred" => conn.execute(
                "UPDATE messages SET is_read = 1 \
                 WHERE account_id = ?1 AND is_starred = 1 AND folder != 'trash'",
                [account_id],
            )?,
            f if STORAGE_FOLDERS.contains(&f) => conn.execute(
                "UPDATE messages SET is_read = 1 WHERE account_id = ?1 AND folder = ?2",
                params![account_id, folder],
            )?,
            _ => return Err(DbError::Invalid(format!("unknown folder: {folder}"))),
        };
        Ok(n)
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_message(
        conn: &Connection,
        account_id: i64,
        folder: &str,
        from_name: &str,
        from_addr: &str,
        to_addrs: &str,
        cc_addrs: &str,
        subject: &str,
        body: &str,
        is_read: bool,
        in_reply_to: Option<i64>,
        created_at: i64,
    ) -> rusqlite::Result<i64> {
        conn.execute(
            "INSERT INTO messages (account_id, folder, from_name, from_addr, to_addrs, cc_addrs, \
                 subject, body, is_read, in_reply_to, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                account_id,
                folder,
                from_name,
                from_addr,
                to_addrs,
                cc_addrs,
                subject,
                body,
                is_read,
                in_reply_to,
                created_at
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Creates or updates a draft.
    pub fn save_draft(&self, draft_id: Option<i64>, c: &Compose) -> DbResult<Message> {
        let account = self.get_account(c.account_id)?;
        let id =
            match draft_id {
                Some(id) => {
                    let existing = self.get_message(id)?;
                    if existing.folder != "drafts" || existing.account_id != c.account_id {
                        return Err(DbError::Invalid(
                            "message is not a draft of this account".into(),
                        ));
                    }
                    self.conn().execute(
                    "UPDATE messages SET to_addrs = ?2, cc_addrs = ?3, subject = ?4, body = ?5, \
                         in_reply_to = ?6, created_at = ?7 WHERE id = ?1",
                    params![id, c.to.trim(), c.cc.trim(), c.subject, c.body, c.in_reply_to, now()],
                )?;
                    id
                }
                None => Self::insert_message(
                    &self.conn(),
                    account.id,
                    "drafts",
                    &account.name,
                    &account.email,
                    c.to.trim(),
                    c.cc.trim(),
                    &c.subject,
                    &c.body,
                    true,
                    c.in_reply_to,
                    now(),
                )?,
            };
        self.get_message(id)
    }

    /// Sends a message: stores a copy in the sender's Sent folder and delivers
    /// it to the inbox of every recipient that is a local account.
    pub fn send(&self, c: &Compose) -> DbResult<SendResult> {
        let account = self.get_account(c.account_id)?;
        let to = parse_addresses(&c.to)?;
        let cc = parse_addresses(&c.cc)?;
        if to.is_empty() && cc.is_empty() {
            return Err(DbError::Invalid(
                "at least one recipient is required".into(),
            ));
        }
        if let Some(draft_id) = c.draft_id {
            let draft = self.get_message(draft_id)?;
            if draft.folder != "drafts" || draft.account_id != account.id {
                return Err(DbError::Invalid(
                    "message is not a draft of this account".into(),
                ));
            }
        }
        let subject = if c.subject.trim().is_empty() {
            "(no subject)".to_string()
        } else {
            c.subject.trim().to_string()
        };
        let to_str = to.join(", ");
        let cc_str = cc.join(", ");
        let created_at = now();

        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let sent_id = Self::insert_message(
            &tx,
            account.id,
            "sent",
            &account.name,
            &account.email,
            &to_str,
            &cc_str,
            &subject,
            &c.body,
            true,
            c.in_reply_to,
            created_at,
        )?;

        let mut seen = HashSet::new();
        let mut delivered_local = 0;
        let mut undelivered = Vec::new();
        for addr in to.iter().chain(cc.iter()) {
            if !seen.insert(addr.clone()) {
                continue;
            }
            let recipient: Option<i64> = tx
                .query_row("SELECT id FROM accounts WHERE email = ?1", [addr], |r| {
                    r.get(0)
                })
                .optional()?;
            match recipient {
                Some(rid) => {
                    Self::insert_message(
                        &tx,
                        rid,
                        "inbox",
                        &account.name,
                        &account.email,
                        &to_str,
                        &cc_str,
                        &subject,
                        &c.body,
                        false,
                        None,
                        created_at,
                    )?;
                    delivered_local += 1;
                }
                None => undelivered.push(addr.clone()),
            }
        }
        if let Some(draft_id) = c.draft_id {
            tx.execute("DELETE FROM messages WHERE id = ?1", [draft_id])?;
        }
        tx.commit()?;
        drop(conn);

        Ok(SendResult {
            message: self.get_message(sent_id)?,
            delivered_local,
            undelivered,
        })
    }

    /// Delivers an externally received message to the given local accounts.
    pub fn deliver_incoming(&self, account_ids: &[i64], m: &Incoming) -> DbResult<Vec<i64>> {
        let created_at = m.created_at.unwrap_or_else(now);
        let subject = if m.subject.trim().is_empty() {
            "(no subject)"
        } else {
            m.subject.trim()
        };
        let mut conn = self.conn();
        let tx = conn.transaction()?;
        let mut ids = Vec::new();
        for &account_id in account_ids {
            ids.push(Self::insert_message(
                &tx,
                account_id,
                "inbox",
                &m.from_name,
                &m.from_addr,
                &m.to_addrs,
                &m.cc_addrs,
                subject,
                &m.body,
                false,
                None,
                created_at,
            )?);
        }
        tx.commit()?;
        Ok(ids)
    }

    /// Populates a fresh database with demo accounts and messages.
    pub fn seed_if_empty(&self) -> DbResult<bool> {
        if !self.list_accounts()?.is_empty() {
            return Ok(false);
        }
        let me = self.create_account("Me", "me@mymail.local")?;
        let alice = self.create_account("Alice Chen", "alice@mymail.local")?;
        let bob = self.create_account("Bob Rivera", "bob@mymail.local")?;
        let mail = |from: &Account, to: &str, subject: &str, body: &str| {
            self.send(&Compose {
                account_id: from.id,
                to: to.into(),
                subject: subject.into(),
                body: body.into(),
                ..Default::default()
            })
        };
        mail(
            &alice,
            &me.email,
            "Welcome to MyMail 👋",
            "Hi!\n\nThis is your local mail app. Everything — the Rust server, the SQLite \
             database and this native macOS client — runs on your machine.\n\n\
             Try it out:\n  • Compose a message to alice@mymail.local or bob@mymail.local\n  \
             • Switch accounts from the sidebar to see it arrive\n  • Star, archive, search and \
             delete messages\n  • Send mail from any tool via SMTP on 127.0.0.1:2525\n\nEnjoy,\nAlice",
        )?;
        mail(
            &bob,
            &format!("{}, {}", me.email, alice.email),
            "Lunch on Friday?",
            "Hey both,\n\nWant to grab lunch on Friday around 12:30? There's a new ramen place \
             near the office.\n\nBob",
        )?;
        let starred = mail(
            &alice,
            &me.email,
            "Project notes",
            "Here are the notes from today:\n\n1. Ship the search feature\n2. Polish the compose \
             window\n3. Write more tests\n\nLet me know if I missed anything.",
        )?;
        // Star the copy in my inbox.
        if let Some(msg) = self
            .list_messages(me.id, "inbox", Some("notes"))?
            .into_iter()
            .find(|m| m.subject == starred.message.subject)
        {
            self.update_message(
                msg.id,
                &MessagePatch {
                    is_starred: Some(true),
                    ..Default::default()
                },
            )?;
        }
        mail(
            &me,
            &alice.email,
            "Re: Project notes",
            "Looks great, thanks! I'll take the search feature.",
        )?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db_with_accounts() -> (Db, Account, Account) {
        let db = Db::open_in_memory().unwrap();
        let a = db.create_account("A", "a@test.local").unwrap();
        let b = db.create_account("B", "B@Test.local").unwrap();
        (db, a, b)
    }

    fn compose(from: i64, to: &str, subject: &str, body: &str) -> Compose {
        Compose {
            account_id: from,
            to: to.into(),
            subject: subject.into(),
            body: body.into(),
            ..Default::default()
        }
    }

    #[test]
    fn parses_addresses() {
        assert_eq!(
            parse_addresses("Alice <Alice@X.io>, bob@y.io; ").unwrap(),
            vec!["alice@x.io", "bob@y.io"]
        );
        assert!(parse_addresses("not-an-address").is_err());
        assert!(parse_addresses("a@").is_err());
        assert!(parse_addresses("").unwrap().is_empty());
    }

    #[test]
    fn account_emails_are_unique_case_insensitive() {
        let (db, _, _) = db_with_accounts();
        assert!(matches!(
            db.create_account("Dup", "A@TEST.LOCAL"),
            Err(DbError::Conflict(_))
        ));
        assert!(matches!(
            db.create_account("", "x@y.z"),
            Err(DbError::Invalid(_))
        ));
        assert!(matches!(
            db.create_account("X", "bad"),
            Err(DbError::Invalid(_))
        ));
    }

    #[test]
    fn send_delivers_locally_and_reports_external() {
        let (db, a, b) = db_with_accounts();
        let res = db
            .send(&compose(
                a.id,
                "b@test.local, ext@example.com",
                "Hi",
                "Hello B",
            ))
            .unwrap();
        assert_eq!(res.delivered_local, 1);
        assert_eq!(res.undelivered, vec!["ext@example.com"]);
        assert_eq!(res.message.folder, "sent");

        let inbox = db.list_messages(b.id, "inbox", None).unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].subject, "Hi");
        assert!(!inbox[0].is_read);
        assert_eq!(db.list_messages(a.id, "sent", None).unwrap().len(), 1);
    }

    #[test]
    fn send_requires_recipient_and_defaults_subject() {
        let (db, a, _) = db_with_accounts();
        assert!(matches!(
            db.send(&compose(a.id, "", "x", "y")),
            Err(DbError::Invalid(_))
        ));
        let res = db.send(&compose(a.id, "b@test.local", "  ", "y")).unwrap();
        assert_eq!(res.message.subject, "(no subject)");
    }

    #[test]
    fn duplicate_recipients_deliver_once() {
        let (db, a, b) = db_with_accounts();
        let mut c = compose(a.id, "b@test.local, B <b@test.local>", "x", "y");
        c.cc = "b@test.local".into();
        assert_eq!(db.send(&c).unwrap().delivered_local, 1);
        assert_eq!(db.list_messages(b.id, "inbox", None).unwrap().len(), 1);
    }

    #[test]
    fn folder_counts_and_starred_view() {
        let (db, a, b) = db_with_accounts();
        db.send(&compose(a.id, "b@test.local", "one", "x")).unwrap();
        db.send(&compose(a.id, "b@test.local", "two", "x")).unwrap();
        let inbox = db.list_messages(b.id, "inbox", None).unwrap();
        db.update_message(
            inbox[0].id,
            &MessagePatch {
                is_read: Some(true),
                is_starred: Some(true),
                folder: None,
            },
        )
        .unwrap();

        let counts = db.folder_counts(b.id).unwrap();
        let get = |f: &str| counts.iter().find(|c| c.folder == f).unwrap().clone();
        assert_eq!((get("inbox").total, get("inbox").unread), (2, 1));
        assert_eq!(get("starred").total, 1);
        assert_eq!(db.list_messages(b.id, "starred", None).unwrap().len(), 1);
    }

    #[test]
    fn delete_moves_to_trash_then_purges() {
        let (db, a, b) = db_with_accounts();
        db.send(&compose(a.id, "b@test.local", "bye", "x")).unwrap();
        let id = db.list_messages(b.id, "inbox", None).unwrap()[0].id;
        assert!(!db.delete_message(id).unwrap());
        assert_eq!(db.get_message(id).unwrap().folder, "trash");
        assert!(db.delete_message(id).unwrap());
        assert!(matches!(db.get_message(id), Err(DbError::NotFound(_))));
    }

    #[test]
    fn empty_trash_only_affects_trash() {
        let (db, a, b) = db_with_accounts();
        db.send(&compose(a.id, "b@test.local", "1", "x")).unwrap();
        db.send(&compose(a.id, "b@test.local", "2", "x")).unwrap();
        let id = db.list_messages(b.id, "inbox", None).unwrap()[0].id;
        db.delete_message(id).unwrap();
        assert_eq!(db.empty_trash(b.id).unwrap(), 1);
        assert_eq!(db.list_messages(b.id, "inbox", None).unwrap().len(), 1);
    }

    #[test]
    fn search_uses_fts_and_escapes_input() {
        let (db, a, b) = db_with_accounts();
        db.send(&compose(
            a.id,
            "b@test.local",
            "Quarterly report",
            "numbers attached",
        ))
        .unwrap();
        db.send(&compose(a.id, "b@test.local", "Lunch", "ramen?"))
            .unwrap();
        let hits = db.list_messages(b.id, "inbox", Some("quart")).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].subject, "Quarterly report");
        assert_eq!(
            db.list_messages(b.id, "inbox", Some("ramen"))
                .unwrap()
                .len(),
            1
        );
        // FTS syntax characters must not cause errors.
        assert!(
            db.list_messages(b.id, "inbox", Some("\"unbalanced AND ("))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            db.list_messages(b.id, "all", Some("lunch")).unwrap().len(),
            1
        );
    }

    #[test]
    fn search_index_follows_deletes() {
        let (db, a, b) = db_with_accounts();
        db.send(&compose(a.id, "b@test.local", "Findme", "x"))
            .unwrap();
        let id = db.list_messages(b.id, "inbox", None).unwrap()[0].id;
        db.delete_message(id).unwrap();
        db.delete_message(id).unwrap();
        assert!(
            db.list_messages(b.id, "trash", Some("findme"))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn drafts_save_update_and_send() {
        let (db, a, b) = db_with_accounts();
        let draft = db
            .save_draft(None, &compose(a.id, "b@test.local", "Draft", "v1"))
            .unwrap();
        assert_eq!(draft.folder, "drafts");
        let draft = db
            .save_draft(
                Some(draft.id),
                &compose(a.id, "b@test.local", "Draft", "v2"),
            )
            .unwrap();
        assert_eq!(draft.body, "v2");

        let mut c = compose(a.id, "b@test.local", "Draft", "v2");
        c.draft_id = Some(draft.id);
        db.send(&c).unwrap();
        assert!(db.list_messages(a.id, "drafts", None).unwrap().is_empty());
        assert_eq!(
            db.list_messages(b.id, "inbox", None).unwrap()[0].snippet,
            "v2"
        );
    }

    #[test]
    fn cannot_move_into_virtual_or_drafts_folder() {
        let (db, a, b) = db_with_accounts();
        db.send(&compose(a.id, "b@test.local", "x", "y")).unwrap();
        let id = db.list_messages(b.id, "inbox", None).unwrap()[0].id;
        let bad = |f: &str| MessagePatch {
            folder: Some(f.into()),
            ..Default::default()
        };
        assert!(db.update_message(id, &bad("starred")).is_err());
        assert!(db.update_message(id, &bad("drafts")).is_err());
        assert_eq!(
            db.update_message(id, &bad("archive")).unwrap().folder,
            "archive"
        );
    }

    #[test]
    fn deleting_account_cascades() {
        let (db, a, b) = db_with_accounts();
        db.send(&compose(a.id, "b@test.local", "x", "y")).unwrap();
        db.delete_account(b.id).unwrap();
        assert!(db.list_messages(b.id, "inbox", None).unwrap().is_empty());
        let n: i64 = db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM messages WHERE account_id = ?1",
                [b.id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn seeds_once() {
        let db = Db::open_in_memory().unwrap();
        assert!(db.seed_if_empty().unwrap());
        assert!(!db.seed_if_empty().unwrap());
        let me = db
            .find_account_by_email("me@mymail.local")
            .unwrap()
            .unwrap();
        assert_eq!(db.list_messages(me.id, "inbox", None).unwrap().len(), 3);
        assert_eq!(db.list_messages(me.id, "starred", None).unwrap().len(), 1);
    }
}
