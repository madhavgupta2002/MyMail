//! Minimal SMTP receiver that delivers incoming mail to local accounts.
//!
//! Supports HELO/EHLO, MAIL FROM, RCPT TO, DATA, RSET, NOOP and QUIT. There is
//! no auth or TLS: it binds to loopback and only accepts local recipients.

use std::time::Duration;

use mail_parser::MessageParser;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

use crate::db::{Db, Incoming, is_valid_address};

pub const MAX_MESSAGE_SIZE: usize = 10 * 1024 * 1024;
const MAX_LINE: u64 = 4096;
const MAX_RECIPIENTS: usize = 100;
const IDLE_TIMEOUT: Duration = Duration::from_secs(300);
const HOSTNAME: &str = "mymail.local";

/// Accepts connections forever, handling each on its own task.
pub async fn serve(listener: TcpListener, db: Db) -> std::io::Result<()> {
    loop {
        let (stream, peer) = listener.accept().await?;
        let db = db.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(stream, db).await {
                tracing::debug!("smtp session with {peer} ended: {e}");
            }
        });
    }
}

#[derive(Default)]
struct Transaction {
    from: Option<String>,
    rcpts: Vec<(i64, String)>,
}

enum Line {
    Data(Vec<u8>),
    /// Line exceeded the length limit; the remainder was discarded.
    TooLong,
    Eof,
}

async fn read_line<R: AsyncBufReadExt + Unpin>(reader: &mut R, max: u64) -> std::io::Result<Line> {
    let mut buf = Vec::new();
    let n = tokio::time::timeout(
        IDLE_TIMEOUT,
        (&mut *reader).take(max).read_until(b'\n', &mut buf),
    )
    .await
    .map_err(|_| std::io::Error::new(std::io::ErrorKind::TimedOut, "idle timeout"))??;
    if n == 0 {
        return Ok(Line::Eof);
    }
    if !buf.ends_with(b"\n") {
        // Discard the rest of the oversized line.
        loop {
            let mut rest = Vec::new();
            let n = (&mut *reader)
                .take(max)
                .read_until(b'\n', &mut rest)
                .await?;
            if n == 0 || rest.ends_with(b"\n") {
                break;
            }
        }
        return Ok(Line::TooLong);
    }
    while buf.last().is_some_and(|b| *b == b'\n' || *b == b'\r') {
        buf.pop();
    }
    Ok(Line::Data(buf))
}

async fn reply<W: AsyncWrite + Unpin>(w: &mut W, text: &str) -> std::io::Result<()> {
    w.write_all(text.as_bytes()).await?;
    w.write_all(b"\r\n").await?;
    w.flush().await
}

/// Extracts the address from `FROM:<addr> PARAMS` / `TO:<addr>`.
fn path_arg<'a>(arg: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = arg.get(..prefix.len())?;
    if !rest.eq_ignore_ascii_case(prefix) {
        return None;
    }
    let rest = arg[prefix.len()..].trim_start();
    let (start, end) = (rest.find('<')?, rest.find('>')?);
    (start < end).then(|| rest[start + 1..end].trim())
}

async fn handle(stream: TcpStream, db: Db) -> std::io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let mut tx = Transaction::default();

    reply(&mut write, &format!("220 {HOSTNAME} ESMTP MyMail ready")).await?;
    loop {
        let line = match read_line(&mut reader, MAX_LINE).await? {
            Line::Eof => return Ok(()),
            Line::TooLong => {
                reply(&mut write, "500 Line too long").await?;
                continue;
            }
            Line::Data(l) => String::from_utf8_lossy(&l).into_owned(),
        };
        let (verb, arg) = line.split_once(' ').unwrap_or((&line, ""));
        let arg = arg.trim();
        match verb.to_ascii_uppercase().as_str() {
            "HELO" => reply(&mut write, &format!("250 {HOSTNAME}")).await?,
            "EHLO" => {
                let msg = format!(
                    "250-{HOSTNAME} greets {arg}\r\n250-SIZE {MAX_MESSAGE_SIZE}\r\n250-8BITMIME\r\n250 SMTPUTF8"
                );
                reply(&mut write, &msg).await?
            }
            "MAIL" => match path_arg(arg, "FROM:") {
                Some(from) if from.is_empty() || is_valid_address(from) => {
                    let too_big = arg.split_whitespace().skip(1).any(|p| {
                        p.get(..5).is_some_and(|k| k.eq_ignore_ascii_case("SIZE="))
                            && p[5..].parse::<usize>().is_ok_and(|n| n > MAX_MESSAGE_SIZE)
                    });
                    if too_big {
                        reply(&mut write, "552 Message size exceeds limit").await?;
                    } else {
                        tx = Transaction {
                            from: Some(from.to_lowercase()),
                            rcpts: Vec::new(),
                        };
                        reply(&mut write, "250 OK").await?;
                    }
                }
                _ => reply(&mut write, "501 Syntax: MAIL FROM:<address>").await?,
            },
            "RCPT" => {
                if tx.from.is_none() {
                    reply(&mut write, "503 Need MAIL command first").await?;
                    continue;
                }
                let Some(addr) = path_arg(arg, "TO:").filter(|a| is_valid_address(a)) else {
                    reply(&mut write, "501 Syntax: RCPT TO:<address>").await?;
                    continue;
                };
                if tx.rcpts.len() >= MAX_RECIPIENTS {
                    reply(&mut write, "452 Too many recipients").await?;
                    continue;
                }
                match db.find_account_by_email(addr) {
                    Ok(Some(account)) => {
                        if !tx.rcpts.iter().any(|(id, _)| *id == account.id) {
                            tx.rcpts.push((account.id, account.email));
                        }
                        reply(&mut write, "250 OK").await?
                    }
                    Ok(None) => reply(&mut write, "550 No such user here").await?,
                    Err(e) => {
                        tracing::error!("smtp recipient lookup failed: {e}");
                        reply(&mut write, "451 Local error in processing").await?
                    }
                }
            }
            "DATA" => {
                if tx.rcpts.is_empty() {
                    reply(&mut write, "503 Need RCPT command first").await?;
                    continue;
                }
                reply(&mut write, "354 End data with <CR><LF>.<CR><LF>").await?;
                let Some(data) = read_data(&mut reader).await? else {
                    reply(&mut write, "552 Message size exceeds limit").await?;
                    tx = Transaction::default();
                    continue;
                };
                let incoming = parse_message(&data, &tx);
                let ids: Vec<i64> = tx.rcpts.iter().map(|(id, _)| *id).collect();
                match db.deliver_incoming(&ids, &incoming) {
                    Ok(ids) => {
                        reply(
                            &mut write,
                            &format!("250 OK: delivered to {} mailbox(es)", ids.len()),
                        )
                        .await?
                    }
                    Err(e) => {
                        tracing::error!("smtp delivery failed: {e}");
                        reply(&mut write, "451 Local error in processing").await?
                    }
                }
                tx = Transaction::default();
            }
            "RSET" => {
                tx = Transaction::default();
                reply(&mut write, "250 OK").await?
            }
            "NOOP" => reply(&mut write, "250 OK").await?,
            "QUIT" => {
                reply(&mut write, &format!("221 {HOSTNAME} closing connection")).await?;
                return Ok(());
            }
            _ => reply(&mut write, "502 Command not implemented").await?,
        }
    }
}

/// Reads the DATA section up to the terminating ".", undoing dot-stuffing.
/// Returns `None` if the message exceeded `MAX_MESSAGE_SIZE` (still consumed).
async fn read_data<R: AsyncBufReadExt + Unpin>(reader: &mut R) -> std::io::Result<Option<Vec<u8>>> {
    let mut data = Vec::new();
    let mut too_big = false;
    loop {
        // Body lines may be long (RFC limit is 1000, but be lenient).
        let line = match read_line(reader, MAX_MESSAGE_SIZE as u64 + 2).await? {
            Line::Eof => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed during DATA",
                ));
            }
            Line::TooLong => {
                too_big = true;
                continue;
            }
            Line::Data(l) => l,
        };
        if line == b"." {
            break;
        }
        if too_big {
            continue;
        }
        let line = line.strip_prefix(b".").unwrap_or(&line);
        if data.len() + line.len() + 2 > MAX_MESSAGE_SIZE {
            too_big = true;
            data = Vec::new();
            continue;
        }
        data.extend_from_slice(line);
        data.extend_from_slice(b"\r\n");
    }
    Ok((!too_big).then_some(data))
}

fn join_addresses(addr: Option<&mail_parser::Address<'_>>) -> String {
    addr.map(|a| {
        a.iter()
            .filter_map(|a| a.address())
            .map(|a| a.to_lowercase())
            .collect::<Vec<_>>()
            .join(", ")
    })
    .unwrap_or_default()
}

fn parse_message(raw: &[u8], tx: &Transaction) -> Incoming {
    let envelope_from = tx.from.clone().unwrap_or_default();
    let envelope_to = || {
        tx.rcpts
            .iter()
            .map(|(_, a)| a.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };

    let Some(msg) = MessageParser::default().parse(raw) else {
        return Incoming {
            from_name: envelope_from.clone(),
            from_addr: envelope_from,
            to_addrs: envelope_to(),
            body: String::from_utf8_lossy(raw).into_owned(),
            ..Default::default()
        };
    };
    let from = msg.from().and_then(|a| a.first());
    let from_addr = from
        .and_then(|a| a.address())
        .map(str::to_lowercase)
        .unwrap_or(envelope_from);
    let from_name = from
        .and_then(|a| a.name())
        .map(str::to_string)
        .filter(|n| !n.trim().is_empty())
        .unwrap_or_else(|| from_addr.clone());
    let to_addrs = Some(join_addresses(msg.to()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(envelope_to);
    Incoming {
        from_name,
        from_addr,
        to_addrs,
        cc_addrs: join_addresses(msg.cc()),
        subject: msg.subject().unwrap_or_default().to_string(),
        body: msg.body_text(0).map(|b| b.into_owned()).unwrap_or_default(),
        created_at: msg.date().map(|d| d.to_timestamp()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_path_args() {
        assert_eq!(path_arg("FROM:<a@b.c> SIZE=10", "FROM:"), Some("a@b.c"));
        assert_eq!(path_arg("from: <A@B.c>", "FROM:"), Some("A@B.c"));
        assert_eq!(path_arg("FROM:<>", "FROM:"), Some(""));
        assert_eq!(path_arg("TO:a@b.c", "TO:"), None);
        assert_eq!(path_arg("", "TO:"), None);
    }

    #[test]
    fn parses_mime_message() {
        let tx = Transaction {
            from: Some("env@x.io".into()),
            rcpts: vec![(1, "me@mymail.local".into())],
        };
        let raw = b"From: \"Carol\" <Carol@X.io>\r\nTo: me@mymail.local\r\nCc: z@x.io\r\n\
Subject: Hello\r\nDate: Tue, 1 Jul 2025 10:00:00 +0000\r\n\r\nBody line\r\n";
        let m = parse_message(raw, &tx);
        assert_eq!(m.from_name, "Carol");
        assert_eq!(m.from_addr, "carol@x.io");
        assert_eq!(m.to_addrs, "me@mymail.local");
        assert_eq!(m.cc_addrs, "z@x.io");
        assert_eq!(m.subject, "Hello");
        assert_eq!(m.body.trim(), "Body line");
        assert_eq!(m.created_at, Some(1751364000));
    }

    #[test]
    fn falls_back_to_envelope() {
        let tx = Transaction {
            from: Some("env@x.io".into()),
            rcpts: vec![(1, "me@mymail.local".into())],
        };
        let m = parse_message(b"Subject: hi\r\n\r\nno from header\r\n", &tx);
        assert_eq!(m.from_addr, "env@x.io");
        assert_eq!(m.to_addrs, "me@mymail.local");
    }
}
