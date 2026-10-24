//! relaunchctl binary — argv picks the mode; `driver` runs the transaction over
//! a held bus session (`bus`).
//!
//!   relaunchctl commit   # held connection: Prepare -> (stdin decision) -> Authorize
//!   relaunchctl reduce   # one-shot: Reduce(base, ops)
//!   relaunchctl purge    # one-shot: `Purge()`
//!   relaunchctl status   # one-shot: `has_pending()` -> {"event":"pending",...}
//!
//! No mutation payload (base, proposal, ops) is ever read from argv; it all
//! arrives on stdin, so `/proc/<pid>/cmdline` carries only the mode word. The
//! transaction id never leaves the process (§5.1).
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        reason = "test code: a failed unwrap/index is a failed assertion"
    )
)]

use relaunch_sys::diag;
use tokio::io::{AsyncWriteExt, BufReader};

use relaunchctl::bus::Session;
use relaunchctl::driver::{run_commit, run_reduce, Outcome};
use relaunchctl::protocol::Event;

async fn connect() -> Result<Session, String> {
    Session::connect().await.map_err(|e| e.to_string())
}

async fn emit(event: &Event) {
    let mut out = tokio::io::stdout();
    let mut line = event.to_line();
    line.push('\n');
    let _ = out.write_all(line.as_bytes()).await;
    let _ = out.flush().await;
}

async fn run_purge() -> Outcome {
    let session = match connect().await {
        Ok(s) => s,
        Err(e) => return failed(format!("cannot reach relaunchd: {e}")).await,
    };
    match session.proxy().purge().await {
        Ok(()) => {
            emit(&Event::Committed {
                config: String::new(),
            })
            .await;
            Outcome::Committed
        }
        Err(e) => failed(format!("purge failed: {e}")).await,
    }
}

async fn run_status() -> Outcome {
    // No service means no open transaction: report false, not an error.
    let pending = match connect().await {
        Ok(s) => match s.proxy().has_pending().await {
            Ok(p) => p,
            Err(e) => return failed(format!("status failed: {e}")).await,
        },
        Err(_) => false,
    };
    emit(&Event::Pending { pending }).await;
    Outcome::Committed
}

async fn failed(message: String) -> Outcome {
    emit(&Event::Error {
        message: message.clone(),
    })
    .await;
    Outcome::Failed(message)
}

#[tokio::main]
async fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let mut input = BufReader::new(tokio::io::stdin());
    let mut out = tokio::io::stdout();
    let outcome = match mode.as_str() {
        "commit" => run_commit(&mut input, &mut out, connect).await,
        "reduce" => run_reduce(&mut input, &mut out, connect).await,
        "purge" => run_purge().await,
        "status" => run_status().await,
        other => {
            diag!(
                "relaunchctl: unknown mode ({} bytes); expected commit|reduce|purge|status",
                other.len()
            );
            std::process::exit(2);
        }
    };
    if let Outcome::Failed(e) = &outcome {
        // The event stream already carried the error to the panel; this is a
        // stderr breadcrumb (never a code or id).
        diag!("relaunchctl: {e}");
    }
    // Exit explicitly. A pending stdin read lives on tokio's blocking pool, and
    // dropping the runtime waits for blocking tasks, so returning from main
    // would hang a finished helper while the panel still holds its pipe open.
    std::process::exit(outcome.exit_code());
}
