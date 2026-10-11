use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::Context;
use mymail_server::{api, db::Db, smtp};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "Usage: mymail-server [--db PATH] [--http ADDR] [--smtp ADDR] [--no-seed]

Options (env fallback in brackets):
  --db PATH     SQLite database file      [MYMAIL_DB]   default: data/mymail.db
  --http ADDR   HTTP API listen address   [MYMAIL_HTTP] default: 127.0.0.1:7878
  --smtp ADDR   SMTP listen address       [MYMAIL_SMTP] default: 127.0.0.1:2525
  --no-seed     Don't create demo data in an empty database";

#[derive(Debug, PartialEq)]
struct Config {
    db: PathBuf,
    http: SocketAddr,
    smtp: SocketAddr,
    seed: bool,
}

fn parse_config(
    args: impl IntoIterator<Item = String>,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Config, String> {
    let mut db = env("MYMAIL_DB").unwrap_or_else(|| "data/mymail.db".into());
    let mut http = env("MYMAIL_HTTP").unwrap_or_else(|| "127.0.0.1:7878".into());
    let mut smtp = env("MYMAIL_SMTP").unwrap_or_else(|| "127.0.0.1:2525".into());
    let mut seed = true;

    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((f, v)) => (f.to_string(), Some(v.to_string())),
            None => (arg, None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("missing value for {flag}"))
        };
        match flag.as_str() {
            "--db" => db = value()?,
            "--http" => http = value()?,
            "--smtp" => smtp = value()?,
            "--no-seed" => seed = false,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Config {
        db: PathBuf::from(db),
        http: http
            .parse()
            .map_err(|_| format!("invalid --http address: {http}"))?,
        smtp: smtp
            .parse()
            .map_err(|_| format!("invalid --smtp address: {smtp}"))?,
        seed,
    })
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutting down");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config = match parse_config(std::env::args().skip(1), |k| std::env::var(k).ok()) {
        Ok(c) => c,
        Err(e) if e.is_empty() => {
            println!("{USAGE}");
            return Ok(());
        }
        Err(e) => {
            eprintln!("error: {e}\n\n{USAGE}");
            std::process::exit(2);
        }
    };

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .with_writer(std::io::stderr)
        .init();

    let db = Db::open(&config.db).with_context(|| format!("opening {}", config.db.display()))?;
    if config.seed && db.seed_if_empty()? {
        tracing::info!("seeded demo accounts and messages");
    }

    let http = TcpListener::bind(config.http)
        .await
        .with_context(|| format!("binding HTTP on {}", config.http))?;
    let smtp_listener = TcpListener::bind(config.smtp)
        .await
        .with_context(|| format!("binding SMTP on {}", config.smtp))?;
    tracing::info!(
        db = %config.db.display(),
        "HTTP API on http://{}, SMTP on {}",
        config.http,
        config.smtp
    );

    let server =
        axum::serve(http, api::router(db.clone())).with_graceful_shutdown(shutdown_signal());
    tokio::select! {
        r = server => r.context("HTTP server failed")?,
        r = smtp::serve(smtp_listener, db) => r.context("SMTP server failed")?,
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults() {
        let c = parse_config(args(&[]), |_| None).unwrap();
        assert_eq!(c.db, PathBuf::from("data/mymail.db"));
        assert_eq!(c.http.to_string(), "127.0.0.1:7878");
        assert_eq!(c.smtp.to_string(), "127.0.0.1:2525");
        assert!(c.seed);
    }

    #[test]
    fn flags_override_env() {
        let env = |k: &str| (k == "MYMAIL_DB").then(|| "/env.db".to_string());
        assert_eq!(
            parse_config(args(&[]), env).unwrap().db,
            PathBuf::from("/env.db")
        );
        let c = parse_config(
            args(&[
                "--db",
                "/x.db",
                "--http=127.0.0.1:1",
                "--smtp",
                "127.0.0.1:2",
                "--no-seed",
            ]),
            env,
        )
        .unwrap();
        assert_eq!(c.db, PathBuf::from("/x.db"));
        assert_eq!(c.http.port(), 1);
        assert_eq!(c.smtp.port(), 2);
        assert!(!c.seed);
    }

    #[test]
    fn rejects_bad_args() {
        assert!(parse_config(args(&["--bogus"]), |_| None).is_err());
        assert!(parse_config(args(&["--db"]), |_| None).is_err());
        assert!(parse_config(args(&["--http", "nope"]), |_| None).is_err());
    }
}
