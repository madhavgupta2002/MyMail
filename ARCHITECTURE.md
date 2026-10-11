# MyMail — Architecture

A fully local email app: native macOS SwiftUI client + Rust backend + SQLite.

```
┌──────────────────────┐  HTTP/JSON 127.0.0.1:7878  ┌──────────────────────────┐
│  MyMail.app (SwiftUI)│ ─────────────────────────▶ │ mymail-server (Rust)     │
│  app/                │   spawns + stops server    │ backend/                 │
└──────────────────────┘                            │  axum REST API           │
                                                    │  SMTP receiver :2525     │
  any SMTP client ── SMTP 127.0.0.1:2525 ─────────▶ │  rusqlite (bundled, FTS5)│
                                                    └────────────┬─────────────┘
                                                                 ▼
                                                        data/mymail.db (SQLite)
```

## Layout

| Path | What |
|---|---|
| `backend/` | Rust crate `mymail-server` (lib + bin). `db.rs` storage, `api.rs` HTTP routes, `smtp.rs` SMTP receiver, `main.rs` config/startup |
| `app/` | SwiftPM package, executable target `MyMail` (macOS 14+, SwiftUI) |
| `scripts/build.sh` | Builds both, assembles `build/MyMail.app` (server binary in `Contents/Resources/`) |
| `scripts/run.sh` | Build + launch |
| `data/` | SQLite DB (created at runtime) |

## Mail model

- Accounts are local mailboxes (`me@mymail.local`, `alice@…`, `bob@…` seeded on first run).
- Sending stores a copy in the sender's **sent** folder and delivers to the **inbox** of every recipient that is a local account. Non-local recipients are reported back as `undelivered` (no outbound relay).
- The SMTP receiver accepts mail for local accounts (550 for unknown recipients) and puts it in their inbox.
- Folders: `inbox`, `starred` (virtual: starred and not in trash), `sent`, `drafts`, `archive`, `trash`.
- Deleting moves to trash. Deleting from trash, or deleting a draft, removes it permanently.

## Backend process

```
mymail-server [--db PATH] [--http ADDR] [--smtp ADDR] [--no-seed]
```
Env fallbacks: `MYMAIL_DB`, `MYMAIL_HTTP`, `MYMAIL_SMTP`. Defaults: `data/mymail.db`, `127.0.0.1:7878`, `127.0.0.1:2525`.
Binds loopback only. Seeds demo data into an empty DB. Shuts down cleanly on SIGINT/SIGTERM.

## HTTP API contract

JSON bodies use snake_case. Timestamps are **unix seconds** (`i64`). Errors return `{"error": "message"}` with status 400 (invalid), 404 (not found), 409 (conflict), or 500.

### Types

```jsonc
Account        { "id": 1, "name": "Me", "email": "me@mymail.local", "created_at": 1759200000 }
FolderCount    { "folder": "inbox", "total": 3, "unread": 2 }
MessageSummary { "id", "account_id", "folder", "from_name", "from_addr", "to_addrs",
                 "subject", "snippet", "is_read", "is_starred", "created_at" }
Message        { "id", "account_id", "folder", "from_name", "from_addr", "to_addrs", "cc_addrs",
                 "subject", "body", "is_read", "is_starred", "in_reply_to": null|int, "created_at" }
Compose        { "account_id": 1, "to": "a@x, B <b@y>", "cc": "", "subject": "", "body": "",
                 "in_reply_to": null|int, "draft_id": null|int }   // all but account_id optional
SendResult     { "message": Message, "delivered_local": 1, "undelivered": ["ext@example.com"] }
```

`to_addrs` / `cc_addrs` are comma+space separated lowercase addresses (drafts keep the raw text).

### Endpoints

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/health` | – | `{"status":"ok"}` |
| GET | `/api/accounts` | – | `[Account]` |
| POST | `/api/accounts` | `{name, email}` | 201 `Account` (409 if the email exists) |
| DELETE | `/api/accounts/{id}` | – | 204 |
| GET | `/api/accounts/{id}/folders` | – | `[FolderCount]`, fixed order: inbox, starred, sent, drafts, archive, trash |
| POST | `/api/accounts/{id}/folders/{folder}/read` | – | `{"updated": n}` (mark all read) |
| POST | `/api/accounts/{id}/trash/empty` | – | `{"deleted": n}` |
| GET | `/api/messages?account_id=&folder=&q=` | – | `[MessageSummary]` newest first. `folder` defaults to `inbox`; `all` = everything except trash. `q` = full-text prefix search |
| GET | `/api/messages/{id}` | – | `Message` |
| PATCH | `/api/messages/{id}` | `{is_read?, is_starred?, folder?}` | `Message` |
| DELETE | `/api/messages/{id}` | – | `{"permanent": bool}` |
| POST | `/api/send` | `Compose` | 201 `SendResult` |
| POST | `/api/drafts` | `Compose` | 201 `Message` (new draft) |
| PUT | `/api/drafts/{id}` | `Compose` | `Message` (updated draft) |

## Frontend

- SwiftUI `NavigationSplitView`: sidebar (account switcher + folders with unread badges) → message list (search, unread dot, star, snippet, relative date) → reader (header, body, toolbar: reply, reply all, forward, star, mark unread, archive, delete).
- Compose window/sheet: to, cc, subject, body. Send, save draft, discard. Editing a draft reopens it in compose.
- `APIClient` (async/await URLSession, snake_case decoding, `secondsSince1970` dates) + `@MainActor` `MailStore` (ObservableObject). Polls every 5 s for new mail.
- **Server lifecycle** (`ServerManager`): on launch, check `GET /api/health`. If unreachable, spawn the server binary with `--db <dataDir>/mymail.db`, then wait for health. On quit, terminate the child it spawned.
  - Binary: env `MYMAIL_SERVER_BIN`, else `Bundle.main` resource `mymail-server`, else `../backend/target/release/mymail-server` relative to the working directory.
  - Data dir: env `MYMAIL_DATA_DIR`, else Info.plist key `MyMailDataDir` (written by `scripts/build.sh`), else `~/Library/Application Support/MyMail`.
