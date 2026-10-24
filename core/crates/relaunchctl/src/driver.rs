//! The commit and reduce transactions, generic over the service and the pipes so
//! the lifecycle is testable without a system bus.
//!
//! `run_commit` holds one service connection (spec §5.1) from Prepare, across
//! the human pause, through Authorize. Input stays watched DURING Authorize
//! (Sol #4): a cancel, EOF, oversize or malformed frame while polkit is pending
//! withdraws the transaction — the function returns without emitting
//! `committed`, and the caller drops the connection, which the service observes
//! as a disconnect and invalidates. A duplicate `authorize` is ignored. When
//! Authorize completes, the pending input read is dropped and the function
//! returns at once, whether or not the parent still holds the pipe open.

use std::future::Future;

use relaunch_core::{MAX_CLASS_LEN, MAX_REDUCE_OPS, MAX_REQUEST_BYTES};
use tokio::io::{AsyncBufRead, AsyncWrite, AsyncWriteExt};

use crate::frame::{read_frame, Frame};
use crate::protocol::{
    parse_decision, parse_prepare, parse_reduce, Decision, Event, MAX_DECISION_FRAME,
    MAX_REQUEST_FRAME,
};

/// The authority service as the driver needs it. `Session` implements it over
/// D-Bus; tests implement it in memory.
#[allow(
    async_fn_in_trait,
    reason = "only used through concrete generic types in this crate, never as dyn or across threads"
)]
pub trait Service {
    /// `(txid, code, summary, rendered_diff)`; an empty `txid` means an empty
    /// diff.
    async fn prepare(
        &self,
        base: Vec<u8>,
        proposal: Vec<u8>,
    ) -> Result<(Vec<u8>, String, String, String), String>;
    /// Run the credential check and, on success, return the signed document.
    async fn authorize(&self, txid: Vec<u8>) -> Result<Vec<u8>, String>;
    /// Apply reducing ops to the verified base and return the signed document.
    async fn reduce(&self, base: Vec<u8>, ops: Vec<(String, String)>) -> Result<Vec<u8>, String>;
}

/// How a transaction ended, as the process exit status reports it.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// A signed document was produced and emitted.
    Committed,
    /// The diff was empty; nothing to authorize.
    Nothing,
    /// The panel withdrew (cancel, EOF, oversize/malformed frame). Nothing emitted.
    Cancelled,
    /// An error, already emitted as an `error` event.
    Failed(String),
}

impl Outcome {
    /// The process exit status: 1 for `Failed`, else 0.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            Outcome::Failed(_) => 1,
            _ => 0,
        }
    }
}

async fn emit<W: AsyncWrite + Unpin>(out: &mut W, event: &Event) {
    let mut line = event.to_line();
    line.push('\n');
    // A write failure means the panel is gone; the outcome is decided by the
    // caller's state, not by whether the panel heard it.
    let _ = out.write_all(line.as_bytes()).await;
    let _ = out.flush().await;
}

async fn fail<W: AsyncWrite + Unpin>(out: &mut W, message: String) -> Outcome {
    let message = relaunch_core::bounded_message(&message);
    emit(
        out,
        &Event::Error {
            message: message.clone(),
        },
    )
    .await;
    Outcome::Failed(message)
}

fn committed_config(signed: Vec<u8>) -> Result<String, String> {
    String::from_utf8(signed).map_err(|_| "signed document was not valid UTF-8".to_string())
}

/// Read the single request frame of a commit or reduce.
async fn request_frame<R: AsyncBufRead + Unpin>(input: &mut R) -> Result<Vec<u8>, String> {
    match read_frame(input, MAX_REQUEST_FRAME).await {
        Ok(Frame::Line(l)) => Ok(l),
        Ok(Frame::Eof) => Err("no request on stdin".into()),
        Ok(Frame::TooLong) => Err("request too large".into()),
        Err(e) => Err(format!("stdin: {e}")),
    }
}

/// Run one expanding transaction: read the prepare frame, Prepare, emit the
/// review, wait for the decision, Authorize while watching for withdrawal.
pub async fn run_commit<R, W, S, C, F>(input: &mut R, out: &mut W, connect: C) -> Outcome
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    S: Service,
    C: FnOnce() -> F,
    F: Future<Output = Result<S, String>>,
{
    let frame = match request_frame(input).await {
        Ok(f) => f,
        Err(e) => return fail(out, e).await,
    };
    let req = match parse_prepare(&frame) {
        Ok(r) => r,
        Err(e) => return fail(out, e).await,
    };
    let base = req.base.into_bytes();
    let proposal = req.proposal.get().as_bytes().to_vec();
    if base.len() > MAX_REQUEST_BYTES || proposal.len() > MAX_REQUEST_BYTES {
        return fail(out, "request too large".into()).await;
    }

    let svc = match connect().await {
        Ok(s) => s,
        Err(e) => return fail(out, format!("cannot reach relaunchd: {e}")).await,
    };
    // The pairing code stays here: the binding is the txid on this connection,
    // and no review surface shows the code.
    let (txid, _code, summary, diff) = match svc.prepare(base, proposal).await {
        Ok(v) => v,
        Err(e) => return fail(out, format!("prepare refused: {e}")).await,
    };
    if txid.is_empty() {
        emit(out, &Event::Nothing).await;
        return Outcome::Nothing;
    }
    emit(out, &Event::Prepared { summary, diff }).await;

    // The human pause. Only a complete, well-formed `authorize` proceeds; EOF
    // (including an unterminated frame) or an oversize frame is a withdrawal.
    match read_frame(input, MAX_DECISION_FRAME).await {
        Ok(Frame::Line(l)) => match parse_decision(&l) {
            Ok(Decision::Authorize) => {}
            Ok(Decision::Cancel) => return Outcome::Cancelled,
            Err(e) => return fail(out, e).await,
        },
        _ => return Outcome::Cancelled,
    }

    let auth = svc.authorize(txid);
    tokio::pin!(auth);
    loop {
        tokio::select! {
            // Input first: if a cancel and the result are both ready, withdraw.
            biased;
            frame = read_frame(input, MAX_DECISION_FRAME) => match frame {
                // A duplicate authorize is ignored: keep waiting.
                Ok(Frame::Line(l)) if parse_decision(&l) == Ok(Decision::Authorize) => {}
                _ => return Outcome::Cancelled,
            },
            result = &mut auth => {
                return match result.and_then(committed_config) {
                    Ok(config) => {
                        emit(out, &Event::Committed { config }).await;
                        Outcome::Committed
                    }
                    Err(e) => fail(out, format!("authorize failed: {e}")).await,
                };
            }
        }
    }
}

/// Run one promptless reducing transaction from a single request frame.
pub async fn run_reduce<R, W, S, C, F>(input: &mut R, out: &mut W, connect: C) -> Outcome
where
    R: AsyncBufRead + Unpin,
    W: AsyncWrite + Unpin,
    S: Service,
    C: FnOnce() -> F,
    F: Future<Output = Result<S, String>>,
{
    let frame = match request_frame(input).await {
        Ok(f) => f,
        Err(e) => return fail(out, e).await,
    };
    let req = match parse_reduce(&frame) {
        Ok(r) => r,
        Err(e) => return fail(out, e).await,
    };
    // Bound the collection before building the derived request vector.
    if req.base.len() > MAX_REQUEST_BYTES || req.ops.len() > MAX_REDUCE_OPS {
        return fail(out, "request too large".into()).await;
    }
    if req.ops.iter().any(|o| o.class.len() > MAX_CLASS_LEN) {
        return fail(out, "class too long".into()).await;
    }
    let ops: Vec<(String, String)> = req
        .ops
        .into_iter()
        .map(|o| (o.op.as_str().to_string(), o.class))
        .collect();

    let svc = match connect().await {
        Ok(s) => s,
        Err(e) => return fail(out, format!("cannot reach relaunchd: {e}")).await,
    };
    match svc
        .reduce(req.base.into_bytes(), ops)
        .await
        .and_then(committed_config)
    {
        Ok(config) => {
            emit(out, &Event::Committed { config }).await;
            Outcome::Committed
        }
        Err(e) => fail(out, format!("reduce failed: {e}")).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::io::{duplex, AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
    use tokio::sync::Semaphore;

    #[derive(Default)]
    struct Calls {
        connected: AtomicBool,
        prepares: AtomicUsize,
        authorizes: AtomicUsize,
        reduces: AtomicUsize,
    }

    /// In-memory service. `authorize` blocks until the test adds a permit to
    /// `release`, so a test can act while polkit is "pending".
    struct Mock {
        calls: Arc<Calls>,
        txid: Vec<u8>,
        release: Arc<Semaphore>,
    }

    impl Service for Mock {
        async fn prepare(
            &self,
            _b: Vec<u8>,
            _p: Vec<u8>,
        ) -> Result<(Vec<u8>, String, String, String), String> {
            self.calls.prepares.fetch_add(1, Ordering::SeqCst);
            Ok((
                self.txid.clone(),
                "4F2A".into(),
                "Relaunch will open this app at login:".into(),
                "+ added x".into(),
            ))
        }
        async fn authorize(&self, txid: Vec<u8>) -> Result<Vec<u8>, String> {
            self.calls.authorizes.fetch_add(1, Ordering::SeqCst);
            assert_eq!(txid, self.txid);
            let _p = self.release.acquire().await.unwrap();
            Ok(br#"{"v":2}"#.to_vec())
        }
        async fn reduce(&self, _b: Vec<u8>, _o: Vec<(String, String)>) -> Result<Vec<u8>, String> {
            self.calls.reduces.fetch_add(1, Ordering::SeqCst);
            Ok(br#"{"v":2}"#.to_vec())
        }
    }

    struct Harness {
        calls: Arc<Calls>,
        release: Arc<Semaphore>,
        /// Panel side: what the panel writes to the helper's stdin.
        to_helper: DuplexStream,
        /// Panel side: what the helper writes to stdout.
        from_helper: BufReader<DuplexStream>,
        task: tokio::task::JoinHandle<Outcome>,
    }

    fn start_commit(txid: &[u8]) -> Harness {
        let calls = Arc::new(Calls::default());
        let release = Arc::new(Semaphore::new(0));
        let (to_helper, helper_in) = duplex(1 << 16);
        let (helper_out, from_helper) = duplex(1 << 16);
        let mock = Mock {
            calls: calls.clone(),
            txid: txid.to_vec(),
            release: release.clone(),
        };
        let c2 = calls.clone();
        let task = tokio::spawn(async move {
            let mut input = BufReader::new(helper_in);
            let mut out = helper_out;
            run_commit(&mut input, &mut out, move || async move {
                c2.connected.store(true, Ordering::SeqCst);
                Ok(mock)
            })
            .await
        });
        Harness {
            calls,
            release,
            to_helper,
            from_helper: BufReader::new(from_helper),
            task,
        }
    }

    impl Harness {
        async fn send(&mut self, s: &str) {
            self.to_helper.write_all(s.as_bytes()).await.unwrap();
        }
        async fn next_event(&mut self) -> serde_json::Value {
            let mut line = String::new();
            tokio::time::timeout(
                Duration::from_secs(5),
                self.from_helper.read_line(&mut line),
            )
            .await
            .expect("event")
            .unwrap();
            serde_json::from_str(&line).expect("json event")
        }
        async fn outcome(self) -> (Outcome, Arc<Calls>, String) {
            let o = tokio::time::timeout(Duration::from_secs(5), self.task)
                .await
                .expect("driver must return")
                .unwrap();
            // Drain whatever the helper still wrote.
            let mut rest = String::new();
            let mut r = self.from_helper;
            let _ = tokio::time::timeout(Duration::from_millis(50), async {
                let mut l = String::new();
                while r.read_line(&mut l).await.unwrap_or(0) > 0 {
                    rest.push_str(&l);
                    l.clear();
                }
            })
            .await;
            (o, self.calls, rest)
        }
    }

    const PREPARE: &str = "{\"base\":\"\",\"proposal\":{\"entries\":[]}}\n";

    #[tokio::test]
    async fn happy_path_prepares_then_commits_on_authorize() {
        let mut h = start_commit(b"0123456789abcdef");
        h.send(PREPARE).await;
        let ev = h.next_event().await;
        assert_eq!(ev["event"], "prepared");
        assert_eq!(ev["summary"], "Relaunch will open this app at login:");
        assert_eq!(ev["diff"], "+ added x");
        assert!(
            ev.get("code").is_none(),
            "no surface shows the pairing code"
        );
        h.send("{\"decision\":\"authorize\"}\n").await;
        h.release.add_permits(1);
        assert_eq!(h.next_event().await["event"], "committed");
        let (o, calls, _) = h.outcome().await;
        assert_eq!(o, Outcome::Committed);
        assert_eq!(calls.authorizes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn success_returns_while_the_parent_still_holds_stdin_open() {
        // Sol #4: the helper must not wait on another stdin read after success.
        let mut h = start_commit(b"0123456789abcdef");
        h.send(PREPARE).await;
        h.next_event().await;
        h.send("{\"decision\":\"authorize\"}\n").await;
        h.release.add_permits(1);
        // `to_helper` is still alive (pipe open) when we wait for the outcome.
        let (o, _, rest) = h.outcome().await;
        assert_eq!(o, Outcome::Committed);
        assert!(rest.contains("committed"));
    }

    #[tokio::test]
    async fn empty_txid_reports_nothing_and_never_authorizes() {
        let mut h = start_commit(b"");
        h.send(PREPARE).await;
        assert_eq!(h.next_event().await["event"], "nothing");
        let (o, calls, _) = h.outcome().await;
        assert_eq!(o, Outcome::Nothing);
        assert_eq!(calls.authorizes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn eof_before_a_decision_cancels() {
        let mut h = start_commit(b"0123456789abcdef");
        h.send(PREPARE).await;
        h.next_event().await;
        h.to_helper.shutdown().await.unwrap();
        let (o, calls, rest) = h.outcome().await;
        assert_eq!(o, Outcome::Cancelled);
        assert_eq!(calls.authorizes.load(Ordering::SeqCst), 0);
        assert!(rest.is_empty());
    }

    #[tokio::test]
    async fn unterminated_authorize_at_eof_is_not_a_decision() {
        // Sol #13: EOF after an unterminated Authorize object is cancellation.
        let mut h = start_commit(b"0123456789abcdef");
        h.send(PREPARE).await;
        h.next_event().await;
        h.send("{\"decision\":\"authorize\"}").await;
        h.to_helper.shutdown().await.unwrap();
        let (o, calls, _) = h.outcome().await;
        assert_eq!(o, Outcome::Cancelled);
        assert_eq!(calls.authorizes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn decision_with_extra_field_is_refused() {
        let mut h = start_commit(b"0123456789abcdef");
        h.send(PREPARE).await;
        h.next_event().await;
        h.send("{\"decision\":\"authorize\",\"x\":1}\n").await;
        assert_eq!(h.next_event().await["event"], "error");
        let (o, calls, _) = h.outcome().await;
        assert!(matches!(o, Outcome::Failed(_)));
        assert_eq!(calls.authorizes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn oversize_decision_frame_cancels() {
        let mut h = start_commit(b"0123456789abcdef");
        h.send(PREPARE).await;
        h.next_event().await;
        let big = format!("{}\n", " ".repeat(MAX_DECISION_FRAME + 1));
        h.send(&big).await;
        let (o, calls, _) = h.outcome().await;
        assert_eq!(o, Outcome::Cancelled);
        assert_eq!(calls.authorizes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn cancel_while_authorize_is_pending_withdraws_without_committing() {
        // Sol #4: cancellation must win against an in-flight authorization.
        let mut h = start_commit(b"0123456789abcdef");
        h.send(PREPARE).await;
        h.next_event().await;
        h.send("{\"decision\":\"authorize\"}\n").await;
        // Authorize is now awaiting polkit (no permit). The user cancels.
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.send("{\"decision\":\"cancel\"}\n").await;
        let release = h.release.clone();
        let (o, calls, rest) = h.outcome().await;
        // A late success after the withdrawal must change nothing.
        release.add_permits(1);
        assert_eq!(o, Outcome::Cancelled);
        assert_eq!(calls.authorizes.load(Ordering::SeqCst), 1);
        assert!(
            !rest.contains("committed"),
            "a withdrawn txn emitted: {rest}"
        );
    }

    #[tokio::test]
    async fn eof_while_authorize_is_pending_withdraws() {
        let mut h = start_commit(b"0123456789abcdef");
        h.send(PREPARE).await;
        h.next_event().await;
        h.send("{\"decision\":\"authorize\"}\n").await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.to_helper.shutdown().await.unwrap();
        let (o, _, rest) = h.outcome().await;
        assert_eq!(o, Outcome::Cancelled);
        assert!(!rest.contains("committed"));
    }

    #[tokio::test]
    async fn duplicate_authorize_is_ignored_not_doubled() {
        let mut h = start_commit(b"0123456789abcdef");
        h.send(PREPARE).await;
        h.next_event().await;
        h.send("{\"decision\":\"authorize\"}\n{\"decision\":\"authorize\"}\n")
            .await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        h.release.add_permits(1);
        let (o, calls, rest) = h.outcome().await;
        assert_eq!(o, Outcome::Committed);
        assert_eq!(calls.authorizes.load(Ordering::SeqCst), 1);
        assert_eq!(rest.matches("committed").count(), 1);
    }

    #[tokio::test]
    async fn malformed_prepare_never_reaches_the_bus() {
        let mut h = start_commit(b"0123456789abcdef");
        h.send("{\"proposal\":{},\"txid\":\"x\"}\n").await;
        assert_eq!(h.next_event().await["event"], "error");
        let (o, calls, _) = h.outcome().await;
        assert!(matches!(o, Outcome::Failed(_)));
        assert!(!calls.connected.load(Ordering::SeqCst));
    }

    async fn reduce_with(line: String) -> (Outcome, Arc<Calls>) {
        let calls = Arc::new(Calls::default());
        let mock = Mock {
            calls: calls.clone(),
            txid: vec![],
            release: Arc::new(Semaphore::new(0)),
        };
        let mut input = BufReader::new(line.as_bytes());
        let mut out = Vec::new();
        let c2 = calls.clone();
        let o = run_reduce(&mut input, &mut out, move || async move {
            c2.connected.store(true, Ordering::SeqCst);
            Ok(mock)
        })
        .await;
        (o, calls)
    }

    #[tokio::test]
    async fn reduce_happy_path() {
        let (o, calls) =
            reduce_with("{\"base\":\"x\",\"ops\":[{\"op\":\"drop\",\"class\":\"a\"}]}\n".into())
                .await;
        assert_eq!(o, Outcome::Committed);
        assert_eq!(calls.reduces.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn reduce_bounds_ops_and_class_before_the_bus() {
        let op = "{\"op\":\"drop\",\"class\":\"a\"}";
        let many = vec![op; MAX_REDUCE_OPS + 1].join(",");
        let (o, calls) = reduce_with(format!("{{\"ops\":[{many}]}}\n")).await;
        assert!(matches!(o, Outcome::Failed(_)));
        assert!(!calls.connected.load(Ordering::SeqCst));

        let long = "c".repeat(MAX_CLASS_LEN + 1);
        let (o, calls) = reduce_with(format!(
            "{{\"ops\":[{{\"op\":\"drop\",\"class\":\"{long}\"}}]}}\n"
        ))
        .await;
        assert!(matches!(o, Outcome::Failed(_)));
        assert!(!calls.connected.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn reduce_without_newline_is_not_a_request() {
        let (o, calls) = reduce_with("{\"ops\":[]}".into()).await;
        assert!(matches!(o, Outcome::Failed(_)));
        assert!(!calls.connected.load(Ordering::SeqCst));
    }
}
