//! `viv-hub`: runs the service, and lets the person who runs it issue and revoke tokens.
//!
//! - `viv-hub serve [--db FILE] [--addr HOST:PORT] [--tls-in-front]`
//! - `viv-hub token issue <user> [--db FILE] [--days N]`
//! - `viv-hub token revoke <user> [--db FILE]`

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use vivianne_hub::server;
use vivianne_hub::sessions::valid_user;
use vivianne_hub::sqlite::Database;

const USAGE: &str = "usage: viv-hub serve [--db FILE] [--addr HOST:PORT] [--tls-in-front]
       viv-hub token issue <user> [--db FILE] [--days N]
       viv-hub token revoke <user> [--db FILE]";
const DEFAULT_ADDR: &str = "127.0.0.1:8787";
const DEFAULT_DAYS: u64 = 30;
const MAX_DAYS: u64 = 365;

enum Failed {
    Usage(String),
    Run(String),
}

fn value(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, Failed> {
    let Some(at) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    if at + 1 >= args.len() {
        return Err(Failed::Usage(format!("{flag} needs a value")));
    }
    let value = args.remove(at + 1);
    args.remove(at);
    Ok(Some(value))
}

fn switch(args: &mut Vec<String>, flag: &str) -> bool {
    let before = args.len();
    args.retain(|a| a != flag);
    args.len() != before
}

fn open(path: &Path) -> Result<Database, Failed> {
    Database::open(path).map_err(|e| Failed::Run(e.to_string()))
}

async fn run(mut args: Vec<String>) -> Result<(), Failed> {
    let db = PathBuf::from(value(&mut args, "--db")?.unwrap_or_else(|| "viv-hub.db".to_owned()));
    let addr = value(&mut args, "--addr")?.unwrap_or_else(|| DEFAULT_ADDR.to_owned());
    let days = value(&mut args, "--days")?;
    let tls_in_front = switch(&mut args, "--tls-in-front");
    let words: Vec<&str> = args.iter().map(String::as_str).collect();
    match words.as_slice() {
        ["serve"] => {
            let addr: SocketAddr = addr
                .parse()
                .map_err(|_| Failed::Usage(format!("`{addr}` is not HOST:PORT")))?;
            // Tokens travel in the clear on plain HTTP, so only this machine may connect unless told otherwise.
            if !addr.ip().is_loopback() && !tls_in_front {
                return Err(Failed::Usage(
                    "listening beyond this machine needs TLS in front; pass --tls-in-front once it is"
                        .to_owned(),
                ));
            }
            let db = Arc::new(open(&db)?);
            let listener = tokio::net::TcpListener::bind(addr)
                .await
                .map_err(|e| Failed::Run(format!("cannot listen on {addr}: {e}")))?;
            eprintln!("viv-hub: listening on {addr}");
            let stop = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            server::serve(listener, db, stop)
                .await
                .map_err(|e| Failed::Run(e.to_string()))
        }
        ["token", "issue", user] => {
            if !valid_user(user) {
                return Err(Failed::Usage(
                    "a user id is letters, digits, `-`, `_` or `.`, at most 64".to_owned(),
                ));
            }
            let days = days
                .map_or(Ok(DEFAULT_DAYS), |d| d.parse::<u64>())
                .ok()
                .filter(|d| (1..=MAX_DAYS).contains(d))
                .ok_or_else(|| Failed::Usage(format!("--days is 1 to {MAX_DAYS}")))?;
            let now = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|d| u64::try_from(d.as_millis()).ok())
                .ok_or_else(|| Failed::Run("the clock is before 1970".to_owned()))?;
            let token = open(&db)?
                .issue_token(&(*user).into(), now, days * 24 * 60 * 60 * 1000)
                .map_err(|e| Failed::Run(e.to_string()))?;
            // Shown once: only its hash is kept.
            println!("{token}");
            Ok(())
        }
        ["token", "revoke", user] => {
            let count = open(&db)?
                .revoke_user(&(*user).into())
                .map_err(|e| Failed::Run(e.to_string()))?;
            eprintln!("viv-hub: revoked {count} token(s)");
            Ok(())
        }
        _ => Err(Failed::Usage(USAGE.to_owned())),
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run(std::env::args().skip(1).collect()).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failed::Usage(message)) => {
            eprintln!("viv-hub: {message}");
            ExitCode::from(2)
        }
        Err(Failed::Run(message)) => {
            eprintln!("viv-hub: {message}");
            ExitCode::FAILURE
        }
    }
}
