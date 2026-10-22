//! Chunk-2 tests for the authority engine: enrollment + sign/verify, the
//! lease/one-per-uid state machine, S1 (a tampered base is never endorsed),
//! Reduce/Purge invalidation, the base-generation recheck across a concurrent
//! Reduce, and durable gen monotonicity.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ed25519_dalek::VerifyingKey;
use relaunch_core::{parse_and_verify, Entry, ExecSource, Provenance};
use relaunchd::{Authenticator, Clock, Engine, EngineError, Proposal, ReduceOp, Store, StoreError};

// ---- test scaffolding ----

struct TmpDir(PathBuf);
impl TmpDir {
    fn new(tag: &str) -> Self {
        let mut p = std::env::temp_dir();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        p.push(format!(
            "relaunchd-test-{}-{}-{}",
            std::process::id(),
            tag,
            n
        ));
        std::fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
static COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone)]
struct FixedClock(Arc<AtomicU64>);
impl Clock for FixedClock {
    fn now_secs(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

struct TestAuth {
    result: bool,
    calls: Arc<Mutex<Vec<String>>>,
    on_challenge: Option<Box<dyn Fn() + Send + Sync>>,
}
impl TestAuth {
    fn allow() -> Self {
        TestAuth {
            result: true,
            calls: Arc::new(Mutex::new(vec![])),
            on_challenge: None,
        }
    }
    fn deny() -> Self {
        TestAuth {
            result: false,
            calls: Arc::new(Mutex::new(vec![])),
            on_challenge: None,
        }
    }
}
impl Authenticator for TestAuth {
    fn authorize(&self, req: relaunchd::AuthRequest) -> Result<bool, String> {
        self.calls.lock().unwrap().push(req.message.to_string());
        if let Some(cb) = &self.on_challenge {
            cb();
        }
        Ok(self.result)
    }
}

struct NoAuth;
impl Authenticator for NoAuth {
    fn authorize(&self, _req: relaunchd::AuthRequest) -> Result<bool, String> {
        Ok(true)
    }
}

const UID: u32 = 1000;
const USER: &str = "laytonf";

fn entry(class: &str, ws: u32) -> Entry {
    Entry {
        class: class.to_string(),
        workspace: ws,
        float: None,
        label: String::new(),
        exec: format!("run-{class}"),
        exec_source: ExecSource::OverridesTable,
        provenance: Provenance::None,
        launcher: None,
        enabled: true,
    }
}

fn proposal(entries: Vec<Entry>) -> Proposal {
    Proposal {
        stagger_seconds: 0,
        entries,
    }
}

fn engine(dir: &TmpDir, clock: &FixedClock, auth: TestAuth) -> Engine<FixedClock, TestAuth> {
    Engine::new(Store::new(dir.0.clone()), clock.clone(), auth)
}

/// Verify a returned config.json against the uid's committed key at the given gen.
fn verify_returned(dir: &TmpDir, json: &[u8], gen: u64) -> usize {
    let st = Store::new(dir.0.clone()).current(UID).unwrap().unwrap();
    let mut pk = [0u8; 32];
    hex::decode_to_slice(&st.pubkey, &mut pk).unwrap();
    let vk = VerifyingKey::from_bytes(&pk).unwrap();
    let cfg = parse_and_verify(json, &vk, UID, USER, gen).unwrap();
    cfg.entries.len()
}

// ---- tests ----

#[test]
fn first_authorize_enrolls_and_signs_verifiable_config() {
    let dir = TmpDir::new("enroll");
    let clock = FixedClock(Arc::new(AtomicU64::new(100)));
    let eng = engine(&dir, &clock, TestAuth::allow());

    let prep = eng
        .prepare(
            UID,
            USER,
            ":1.1",
            None,
            proposal(vec![entry("brave", 1), entry("zed", 2)]),
        )
        .unwrap();
    assert_eq!(prep.changed, 2);
    let txid = prep.txid.unwrap();
    assert_eq!(prep.code.unwrap().len(), 4);

    let json = eng.authorize(UID, ":1.1", &txid).unwrap();
    // Enrollment committed gen 1, and the signed config verifies at gen 1.
    assert_eq!(verify_returned(&dir, &json, 1), 2);
    assert!(!eng.has_pending(UID));
}

#[test]
fn empty_diff_yields_no_transaction() {
    let dir = TmpDir::new("empty");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng = engine(&dir, &clock, TestAuth::allow());
    // Enroll with one entry.
    let p = eng
        .prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    let json = eng.authorize(UID, ":1.1", &p.txid.unwrap()).unwrap();
    // Re-propose the identical list as the base -> nothing to do.
    let p2 = eng
        .prepare(
            UID,
            USER,
            ":1.1",
            Some(&json),
            proposal(vec![entry("a", 1)]),
        )
        .unwrap();
    assert_eq!(p2.changed, 0);
    assert!(p2.txid.is_none());
    assert!(!eng.has_pending(UID));
}

#[test]
fn one_pending_transaction_per_uid() {
    let dir = TmpDir::new("busy");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng = engine(&dir, &clock, TestAuth::allow());
    eng.prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    let second = eng.prepare(UID, USER, ":1.2", None, proposal(vec![entry("b", 1)]));
    assert!(matches!(second, Err(EngineError::Busy)));
}

#[test]
fn authorize_requires_same_sender() {
    let dir = TmpDir::new("sender");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng = engine(&dir, &clock, TestAuth::allow());
    let p = eng
        .prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    let r = eng.authorize(UID, ":9.9", &p.txid.unwrap());
    assert!(matches!(r, Err(EngineError::WrongSender)));
    // The transaction survives a wrong-sender attempt (still Prepared).
    assert!(eng.has_pending(UID));
}

#[test]
fn expired_lease_frees_slot_and_cannot_authorize() {
    let dir = TmpDir::new("ttl");
    let now = Arc::new(AtomicU64::new(1000));
    let clock = FixedClock(now.clone());
    let eng = Engine::new(Store::new(dir.0.clone()), clock, TestAuth::allow()).with_ttl(180);
    let p = eng
        .prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    let txid = p.txid.unwrap();
    now.store(1000 + 200, Ordering::SeqCst); // past TTL
    assert!(!eng.has_pending(UID));
    assert!(matches!(
        eng.authorize(UID, ":1.1", &txid),
        Err(EngineError::Stale)
    ));
    // Slot is free: a new prepare succeeds.
    assert!(eng
        .prepare(UID, USER, ":1.1", None, proposal(vec![entry("b", 1)]))
        .is_ok());
}

#[test]
fn auth_denied_clears_pending() {
    let dir = TmpDir::new("deny");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng = engine(&dir, &clock, TestAuth::deny());
    let p = eng
        .prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    let r = eng.authorize(UID, ":1.1", &p.txid.unwrap());
    assert!(matches!(r, Err(EngineError::AuthDenied)));
    assert!(!eng.has_pending(UID));
}

#[test]
fn tampered_base_is_treated_as_empty_so_planted_rows_show_as_additions() {
    // S1: a writer plants an enabled row in config.json without the key. When
    // that unverifiable base is handed to prepare, it is discarded, so the
    // planted row appears as an ADDITION the human must approve, never silently
    // carried forward.
    let dir = TmpDir::new("s1");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng = engine(&dir, &clock, TestAuth::allow());
    // Enroll a legitimate single-entry base.
    let good = eng
        .prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    let base = eng.authorize(UID, ":1.1", &good.txid.unwrap()).unwrap();

    // Forge: flip the signed command in config.json without re-signing.
    let mut s = String::from_utf8(base).unwrap();
    s = s.replace("run-a", "evilcommand1");

    // Propose "a" again. Because the tampered base fails verification, the
    // engine diffs against an EMPTY base: "a" is a fresh addition.
    let p = eng
        .prepare(
            UID,
            USER,
            ":1.1",
            Some(s.as_bytes()),
            proposal(vec![entry("a", 1)]),
        )
        .unwrap();
    assert_eq!(
        p.changed, 1,
        "planted/tampered base must not be carried forward"
    );
    assert!(p.rendered_diff.contains("+ class=\"a\""));
    assert!(!p.rendered_diff.contains("evilcommand1"));
}

#[test]
fn reduce_before_authorize_invalidates_the_transaction() {
    let dir = TmpDir::new("reduce-inv");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng = engine(&dir, &clock, TestAuth::allow());
    // Enroll two entries (gen 1).
    let p = eng
        .prepare(
            UID,
            USER,
            ":1.1",
            None,
            proposal(vec![entry("a", 1), entry("b", 1)]),
        )
        .unwrap();
    let base = eng.authorize(UID, ":1.1", &p.txid.unwrap()).unwrap();
    // Prepare an expanding change (gen base 1).
    let p2 = eng
        .prepare(
            UID,
            USER,
            ":1.1",
            Some(&base),
            proposal(vec![entry("a", 3), entry("b", 1)]),
        )
        .unwrap();
    // A reduce lands first: it invalidates the pending transaction.
    let _ = eng
        .reduce(UID, USER, &base, &[ReduceOp::Drop("b".into())])
        .unwrap();
    let r = eng.authorize(UID, ":1.1", &p2.txid.unwrap());
    assert!(matches!(r, Err(EngineError::NoSuchTransaction)));
}

#[test]
fn concurrent_reduce_during_challenge_makes_commit_stale() {
    // Phase-1 claims the transaction, the lock drops for the challenge, a Reduce
    // on a second engine over the SAME store advances the committed gen, and the
    // phase-3 base-generation recheck then refuses to sign.
    let dir = TmpDir::new("stale-gen");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));

    // Enroll via engine1 (gen 1).
    let eng1_enroll = engine(&dir, &clock, TestAuth::allow());
    let p = eng1_enroll
        .prepare(
            UID,
            USER,
            ":1.1",
            None,
            proposal(vec![entry("a", 1), entry("b", 1)]),
        )
        .unwrap();
    let base = eng1_enroll
        .authorize(UID, ":1.1", &p.txid.unwrap())
        .unwrap();

    // engine2 (shares the store dir) performs a reduce when the challenge fires.
    let eng2 = Arc::new(Engine::new(
        Store::new(dir.0.clone()),
        clock.clone(),
        NoAuth,
    ));
    let base_for_cb = base.clone();
    let eng2_cb = eng2.clone();
    let auth = TestAuth {
        result: true,
        calls: Arc::new(Mutex::new(vec![])),
        on_challenge: Some(Box::new(move || {
            eng2_cb
                .reduce(UID, USER, &base_for_cb, &[ReduceOp::Drop("b".into())])
                .unwrap();
        })),
    };
    let eng1 = Engine::new(Store::new(dir.0.clone()), clock, auth);

    let p2 = eng1
        .prepare(
            UID,
            USER,
            ":1.1",
            Some(&base),
            proposal(vec![entry("a", 5), entry("b", 1)]),
        )
        .unwrap();
    let r = eng1.authorize(UID, ":1.1", &p2.txid.unwrap());
    assert!(
        matches!(r, Err(EngineError::Stale)),
        "commit must refuse a superseded base gen"
    );
}

#[test]
fn reduce_disable_and_drop_sign_a_new_generation() {
    let dir = TmpDir::new("reduce-ok");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng = engine(&dir, &clock, TestAuth::allow());
    let p = eng
        .prepare(
            UID,
            USER,
            ":1.1",
            None,
            proposal(vec![entry("a", 1), entry("b", 2)]),
        )
        .unwrap();
    let base = eng.authorize(UID, ":1.1", &p.txid.unwrap()).unwrap();
    let reduced = eng
        .reduce(UID, USER, &base, &[ReduceOp::Drop("a".into())])
        .unwrap();
    // gen advanced to 2, one entry remains, verifies.
    assert_eq!(verify_returned(&dir, &reduced, 2), 1);
}

#[test]
fn reduce_requires_a_verifying_base() {
    let dir = TmpDir::new("reduce-bad");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng = engine(&dir, &clock, TestAuth::allow());
    let p = eng
        .prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    let base = eng.authorize(UID, ":1.1", &p.txid.unwrap()).unwrap();
    let mut s = String::from_utf8(base).unwrap();
    s = s.replace("run-a", "tampered1234");
    let r = eng.reduce(UID, USER, s.as_bytes(), &[ReduceOp::Drop("a".into())]);
    assert!(matches!(r, Err(EngineError::Verify(_))));
}

#[test]
fn purge_removes_state_and_then_reduce_is_not_enrolled() {
    let dir = TmpDir::new("purge");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng = engine(&dir, &clock, TestAuth::allow());
    let p = eng
        .prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    let base = eng.authorize(UID, ":1.1", &p.txid.unwrap()).unwrap();
    eng.purge(UID).unwrap();
    assert!(Store::new(dir.0.clone()).current(UID).unwrap().is_none());
    let r = eng.reduce(UID, USER, &base, &[ReduceOp::Drop("a".into())]);
    assert!(matches!(r, Err(EngineError::NotEnrolled)));
}

#[test]
fn store_rejects_generation_regression() {
    let dir = TmpDir::new("mono");
    let store = Store::new(dir.0.clone());
    store.ensure_enrolled(UID, USER).unwrap();
    let key = store.signing_key(UID).unwrap();
    let pk = key.verifying_key().to_bytes();
    store.publish_gen(UID, USER, 1, &pk).unwrap();
    store.publish_gen(UID, USER, 2, &pk).unwrap();
    // Going backward (or sideways) is refused.
    assert!(store.publish_gen(UID, USER, 2, &pk).is_err());
    assert!(store.publish_gen(UID, USER, 1, &pk).is_err());
}

#[test]
fn message_carries_the_code_and_no_exec_text() {
    let dir = TmpDir::new("msg");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let calls = Arc::new(Mutex::new(vec![]));
    let auth = TestAuth {
        result: true,
        calls: calls.clone(),
        on_challenge: None,
    };
    let eng = Engine::new(Store::new(dir.0.clone()), clock, auth);
    let p = eng
        .prepare(
            UID,
            USER,
            ":1.1",
            None,
            proposal(vec![entry("secretapp", 1)]),
        )
        .unwrap();
    eng.authorize(UID, ":1.1", &p.txid.unwrap()).unwrap();
    let msg = calls.lock().unwrap()[0].clone();
    assert!(msg.contains(&p.code.unwrap()));
    assert!(
        !msg.contains("run-secretapp"),
        "exec text must not reach the polkit message"
    );
}

// ---- store durability: gen-reuse (I1/M1), new enrollment id, purge (I2) ----

#[test]
fn key_only_recovery_after_commit_fails_closed_no_gen0() {
    // Reviewer I1/M1: a retained key whose public.json is gone must NOT be
    // republished at gen 0 (which would re-validate a historical signature).
    let dir = TmpDir::new("gen0");
    let store = Store::new(dir.0.clone());
    store.ensure_enrolled(UID, USER).unwrap(); // key + committed gen 0
    let pk = store.signing_key(UID).unwrap().verifying_key().to_bytes();
    store.publish_gen(UID, USER, 1, &pk).unwrap();
    store.publish_gen(UID, USER, 2, &pk).unwrap();

    std::fs::remove_file(dir.0.join(UID.to_string()).join("public.json")).unwrap();
    let r = store.ensure_enrolled(UID, USER);
    assert!(
        matches!(r, Err(StoreError::Corrupt(_))),
        "must fail closed, got {r:?}"
    );
    assert!(
        store.current(UID).unwrap().is_none(),
        "must not republish gen 0"
    );
}

#[test]
fn purge_then_reenroll_yields_a_new_enrollment_id() {
    // Reviewer M1: a re-enrolled uid gets a NEW key, so a recurring gen number
    // is distinguishable by enrollment id.
    let dir = TmpDir::new("reenroll");
    let store = Store::new(dir.0.clone());
    let eid1 = store.ensure_enrolled(UID, USER).unwrap().enrollment_id;
    store.purge(UID).unwrap();
    let eid2 = store.ensure_enrolled(UID, USER).unwrap().enrollment_id;
    assert_ne!(eid1, eid2);
}

#[test]
fn purge_leaves_no_state_and_no_tombstone() {
    let dir = TmpDir::new("purge-clean");
    let store = Store::new(dir.0.clone());
    store.ensure_enrolled(UID, USER).unwrap();
    store.purge(UID).unwrap();
    assert!(store.current(UID).unwrap().is_none());
    assert!(!dir.0.join(UID.to_string()).exists());
    for ent in std::fs::read_dir(&dir.0).unwrap() {
        let name = ent.unwrap().file_name();
        assert!(
            !name.to_string_lossy().contains("purging"),
            "tombstone left behind"
        );
    }
}

#[test]
fn invalidate_sender_drops_only_that_senders_pending() {
    let dir = TmpDir::new("disc");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng = engine(&dir, &clock, TestAuth::allow());
    eng.prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    assert!(eng.has_pending(UID));
    eng.invalidate_sender(":9.9"); // different connection: no effect
    assert!(eng.has_pending(UID));
    eng.invalidate_sender(":1.1"); // the Prepare connection dropped
    assert!(!eng.has_pending(UID));
}

#[test]
fn invalidate_hook_fires_on_disconnect_and_purge_not_denial() {
    // The transport uses this hook to cancel an in-flight polkit check (M4).
    // Disconnect and Purge can race an in-flight check, so they fire it. A
    // denial/error means the check already RETURNED, so it must NOT fire the
    // hook (reviewer #3), or it would leak a stale id into the cancel set.
    let dir = TmpDir::new("hook");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let rec = Arc::new(Mutex::new(Vec::<u32>::new()));
    let r2 = rec.clone();
    let eng = Engine::new(Store::new(dir.0.clone()), clock, TestAuth::deny())
        .with_invalidate_hook(Arc::new(move |uid, _id| r2.lock().unwrap().push(uid)));

    // Denial does NOT fire the hook.
    let p = eng
        .prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    let _ = eng.authorize(UID, ":1.1", &p.txid.unwrap());
    assert_eq!(
        rec.lock().unwrap().len(),
        0,
        "denial must not fire the cancel hook"
    );

    // Disconnect fires it.
    eng.prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    eng.invalidate_sender(":1.1");
    // Purge fires it.
    eng.purge(UID).unwrap();

    assert_eq!(rec.lock().unwrap().iter().filter(|&&u| u == UID).count(), 2);
}

#[test]
fn oversize_resulting_document_is_refused_without_advancing_gen() {
    // Reviewer #1: the service must not sign a document larger than the shared
    // limit the anchor will read. Enroll a 1-entry base with the default limit,
    // then on a second engine (same store) set the limit to exactly that base's
    // size so any larger resulting document is refused.
    let dir = TmpDir::new("doclimit");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng = engine(&dir, &clock, TestAuth::allow());
    let p = eng
        .prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    let base = eng.authorize(UID, ":1.1", &p.txid.unwrap()).unwrap();

    let clock2 = FixedClock(Arc::new(AtomicU64::new(0)));
    let eng2 = Engine::new(Store::new(dir.0.clone()), clock2, TestAuth::allow())
        .with_doc_limit(base.len());
    let p2 = eng2
        .prepare(
            UID,
            USER,
            ":1.1",
            Some(&base),
            proposal(vec![entry("a", 1), entry("bbbbb", 2)]),
        )
        .unwrap();
    let r = eng2.authorize(UID, ":1.1", &p2.txid.unwrap());
    assert!(
        matches!(r, Err(EngineError::DocumentTooLarge(_))),
        "got {r:?}"
    );
    // Generation did not advance past the enrollment commit.
    assert_eq!(
        Store::new(dir.0.clone()).current(UID).unwrap().unwrap().gen,
        1
    );
}

#[test]
fn pre_dispatch_invalidation_has_no_check_id() {
    // Reviewer M4/#2: an invalidation before the challenge dispatches carries no
    // check id (there is nothing to cancel yet).
    let dir = TmpDir::new("hookid-pre");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let rec = Arc::new(Mutex::new(Vec::<(u32, Option<String>)>::new()));
    let r2 = rec.clone();
    let eng = Engine::new(Store::new(dir.0.clone()), clock, TestAuth::allow())
        .with_invalidate_hook(Arc::new(move |uid, id| r2.lock().unwrap().push((uid, id))));
    eng.prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    eng.invalidate_sender(":1.1");
    let v = rec.lock().unwrap().clone();
    assert_eq!(v, vec![(UID, None)]);
}

#[test]
fn post_dispatch_invalidation_carries_the_check_id() {
    // An invalidation that races an in-flight challenge carries the exact check
    // id. The challenge callback invalidates the sender mid-check (when the
    // check id is already set), reproducing the disconnect-during-prompt case.
    let dir = TmpDir::new("hookid-post");
    let clock = FixedClock(Arc::new(AtomicU64::new(0)));
    let rec = Arc::new(Mutex::new(Vec::<(u32, Option<String>)>::new()));
    let r2 = rec.clone();

    // Late-populated trigger breaks the auth<->engine ownership cycle.
    type Trigger = Arc<Mutex<Option<Box<dyn Fn() + Send>>>>;
    let trigger: Trigger = Arc::new(Mutex::new(None));
    let t2 = trigger.clone();
    let auth = TestAuth {
        result: true,
        calls: Arc::new(Mutex::new(vec![])),
        on_challenge: Some(Box::new(move || {
            if let Some(f) = t2.lock().unwrap().as_ref() {
                f();
            }
        })),
    };
    let eng = Arc::new(
        Engine::new(Store::new(dir.0.clone()), clock, auth)
            .with_invalidate_hook(Arc::new(move |uid, id| r2.lock().unwrap().push((uid, id)))),
    );
    let e2 = eng.clone();
    *trigger.lock().unwrap() = Some(Box::new(move || e2.invalidate_sender(":1.1")));

    let p = eng
        .prepare(UID, USER, ":1.1", None, proposal(vec![entry("a", 1)]))
        .unwrap();
    // The challenge invalidates mid-flight; commit then finds the txn gone.
    let r = eng.authorize(UID, ":1.1", &p.txid.unwrap());
    assert!(matches!(r, Err(EngineError::Stale)), "got {r:?}");
    let v = rec.lock().unwrap().clone();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].0, UID);
    assert!(
        v[0].1.is_some(),
        "post-dispatch invalidation carries the check id"
    );
}
