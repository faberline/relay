//! Streaming work-queue consume (#447/#448) — ONE bidi h2 stream replacing the
//! polling lease/ack/heartbeat dance.
//!
//! The consumer opens the stream and sends `Subscribe{prefetch}`; relay pushes
//! leased entries down (up to the credit window); the consumer sends `Ack`/`Nack`
//! up; each ack frees a credit. Connection drop → the consumer's in-flight leases
//! expire and redeliver via the engine's existing TTL — so exclusivity and
//! redelivery are unchanged; this is purely a streaming wrapper over the
//! work-queue engine. Frames are length-prefixed JSON.

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::body::{Body, BodyDataStream, Bytes};
use axum::extract::{Extension, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use chrono::Utc;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use service_auth::{AuditedRoleMapPrincipal, Role};
use service_http::ApiErr;
use tokio::sync::mpsc;

use crate::server::AppState;
use relay_broker::engine::Relay;

static CONSUMER_SEQ: AtomicU64 = AtomicU64::new(0);

/// Up-frames the consumer sends.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ConsumeUp {
    Subscribe { prefetch: u32 },
    Ack { lease_id: String, epoch: u64 },
    Nack { lease_id: String },
}

/// Down-frame: one leased entry.
#[derive(Debug, Serialize)]
struct LeasedEntry {
    lease_id: String,
    epoch: u64,
    message_id: String,
    payload: serde_json::Value,
}

fn encode_frame<T: Serialize>(v: &T) -> Vec<u8> {
    let body = serde_json::to_vec(v).unwrap_or_default();
    let mut buf = Vec::with_capacity(4 + body.len());
    buf.extend_from_slice(&(body.len() as u32).to_be_bytes());
    buf.extend_from_slice(&body);
    buf
}

#[derive(Default)]
struct Frames {
    buf: Vec<u8>,
}
impl Frames {
    fn push(&mut self, b: &[u8]) {
        self.buf.extend_from_slice(b);
    }
    fn next(&mut self) -> Option<Vec<u8>> {
        if self.buf.len() < 4 {
            return None;
        }
        let n = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if self.buf.len() < 4 + n {
            return None;
        }
        let f = self.buf[4..4 + n].to_vec();
        self.buf.drain(..4 + n);
        Some(f)
    }
}

async fn read_up(stream: &mut BodyDataStream, dec: &mut Frames) -> Option<ConsumeUp> {
    loop {
        if let Some(raw) = dec.next() {
            return serde_json::from_slice(&raw).ok();
        }
        match stream.next().await {
            Some(Ok(chunk)) => dec.push(&chunk),
            _ => return None,
        }
    }
}

/// `POST /v1/{subject}/consume` — bidi streaming work-queue consume: the consumer
/// streams `Subscribe`/`Ack`/`Nack` frames up while relay streams leased entries
/// down (length-prefixed JSON) within a `prefetch` credit window. The primary
/// work-queue consume path (#447/#448), superseding the polling lease/ack/
/// heartbeat routes.
#[utoipa::path(
    post,
    path = "/v1/{subject}/consume",
    params(("subject" = String, Path, description = "Target subject")),
    responses((status = 200, description = "A length-prefixed JSON frame stream of leased entries; the request body streams Subscribe/Ack/Nack frames"))
)]
pub async fn consume(
    State(st): State<AppState>,
    Extension(principal): Extension<AuditedRoleMapPrincipal>,
    Path(subject): Path<String>,
    req: Body,
) -> Response {
    // Consume-family authorization (#1206): a read grant on the subject,
    // checked before the Subscribe handshake is even read.
    if let Err(deny) = relay_access::auth::authorize(&principal, &subject, Role::Read) {
        return deny.into_response();
    }
    let consumer_id = format!("consume-{}", CONSUMER_SEQ.fetch_add(1, Ordering::Relaxed));
    // Read the handshake before the response head goes out: the protocol's
    // first up-frame must be Subscribe{prefetch}. A missing/undecodable first
    // frame is a client error and returns 400 in the shared {error, message}
    // envelope (#1205) instead of the silent empty 200 stream it used to get.
    let mut up = req.into_data_stream();
    let mut dec = Frames::default();
    let prefetch = match read_up(&mut up, &mut dec).await {
        Some(ConsumeUp::Subscribe { prefetch }) => prefetch.max(1),
        _ => {
            return ApiErr::new(
                StatusCode::BAD_REQUEST,
                "bad_request",
                "consume stream must open with a subscribe frame",
            )
            .into_response()
        }
    };
    let (tx, rx) = mpsc::channel::<Vec<u8>>(64);
    tokio::spawn(drive(
        st.relay_handle(),
        st.raft(),
        subject,
        consumer_id,
        prefetch,
        up,
        dec,
        tx,
    ));
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv()
            .await
            .map(|f| (Ok::<Bytes, Infallible>(Bytes::from(f)), rx))
    });
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "application/octet-stream")],
        Body::from_stream(stream),
    )
        .into_response()
}

/// Bookkeeping for leases currently held in-flight by a consume session (#60).
///
/// Tracks `lease_id -> epoch` and the credit count. Ensures prefetch credit
/// is freed only for leases actually delivered by this session, preventing
/// bogus or duplicate acks from pushing in-flight leases past `prefetch`.
#[derive(Default, Debug)]
struct InFlight {
    leases: HashMap<String, u64>,
}

impl InFlight {
    #[cfg(test)]
    fn new() -> Self {
        Self::default()
    }

    fn count(&self) -> u32 {
        self.leases.len() as u32
    }

    #[cfg(test)]
    fn is_empty(&self) -> bool {
        self.leases.is_empty()
    }

    /// Record delivery of a lease; consumes a prefetch credit.
    fn deliver(&mut self, lease_id: impl Into<String>, epoch: u64) {
        self.leases.insert(lease_id.into(), epoch);
    }

    /// Acknowledge a lease. Returns whether a credit was freed (i.e. whether
    /// the lease was actually in-flight for this session).
    fn ack(&mut self, lease_id: &str) -> bool {
        self.leases.remove(lease_id).is_some()
    }

    /// Negative-acknowledge (release) a lease. Returns the epoch if the lease
    /// was in-flight (freeing a credit), or None if it was unknown/already handled.
    fn nack(&mut self, lease_id: &str) -> Option<u64> {
        self.leases.remove(lease_id)
    }

    #[cfg(test)]
    fn epoch(&self, lease_id: &str) -> Option<u64> {
        self.leases.get(lease_id).copied()
    }
}

/// Drive one consume session (post-Subscribe): keep ≤ prefetch entries in
/// flight (lease + push), freeing a credit per Ack/Nack; wake on publish.
async fn drive(
    relay: std::sync::Arc<Relay>,
    raft: Option<std::sync::Arc<relay_replication::raft::RelayRaft>>,
    subject: String,
    consumer_id: String,
    prefetch: u32,
    mut up: BodyDataStream,
    mut dec: Frames,
    tx: mpsc::Sender<Vec<u8>>,
) {
    let mut inflight = InFlight::default();
    // Wake-based push (#465): re-lease the instant a publish or release touches
    // this subject, instead of polling the queue every 50ms while idle. The
    // watch channel tracks the latest revision, so a wake signaled between our
    // drain and the `.changed()` await is not lost.
    let mut wake = relay.subscribe_wake(&subject);
    loop {
        while inflight.count() < prefetch {
            let lease = match &raft {
                Some(raft) => raft
                    .lease(subject.clone(), consumer_id.clone(), Utc::now())
                    .await
                    .ok()
                    .flatten(),
                None => relay
                    .lease(&subject, &consumer_id, Utc::now())
                    .unwrap_or(None),
            };
            match lease {
                Some(l) => {
                    let (message_id, payload) = relay
                        .entry(&l.subject, l.shard, l.seq)
                        .ok()
                        .flatten()
                        .map(|e| (e.message_id, e.payload))
                        .unwrap_or_default();
                    let frame = LeasedEntry {
                        lease_id: l.lease_id.clone(),
                        epoch: l.epoch,
                        message_id,
                        payload,
                    };
                    if tx.send(encode_frame(&frame)).await.is_err() {
                        return;
                    }
                    inflight.deliver(l.lease_id, l.epoch);
                }
                None => break, // queue empty
            }
        }
        tokio::select! {
            up_frame = read_up(&mut up, &mut dec) => match up_frame {
                Some(ConsumeUp::Ack { lease_id, epoch }) => {
                    if inflight.ack(&lease_id) {
                        match &raft {
                            Some(raft) => { let _ = raft.ack(subject.clone(), lease_id.clone(), epoch, Utc::now()).await; }
                            None => { let _ = relay.ack(&subject, &lease_id, Some(epoch)); }
                        }
                    }
                }
                Some(ConsumeUp::Nack { lease_id }) => {
                    // Fast release (#465): reset this lease for immediate
                    // redelivery rather than waiting out the lease TTL.
                    // Only release if this session held the lease (#60); a Nack
                    // has no epoch to fence with otherwise.
                    if let Some(epoch) = inflight.nack(&lease_id) {
                        match &raft {
                            Some(raft) => {
                                let _ = raft.release(subject.clone(), lease_id.clone(), epoch, Utc::now()).await;
                            }
                            None => { let _ = relay.release(&subject, &lease_id); }
                        }
                    }
                }
                Some(ConsumeUp::Subscribe { .. }) => {}
                None => return, // disconnect → in-flight leases expire (TTL redelivery)
            },
            // A publish/release on this subject (or a reconciler reclaim) woke us;
            // loop back and lease whatever is now ready.
            _ = wake.changed() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_codec_and_up_parse() {
        // length-prefixed framing tolerates split reads
        let a = encode_frame(&serde_json::json!({"type": "ack", "lease_id": "L", "epoch": 3}));
        let mut d = Frames::default();
        d.push(&a[..2]);
        assert!(d.next().is_none());
        d.push(&a[2..]);
        match serde_json::from_slice::<ConsumeUp>(&d.next().unwrap()).unwrap() {
            ConsumeUp::Ack { lease_id, epoch } => assert_eq!((lease_id.as_str(), epoch), ("L", 3)),
            _ => panic!("expected Ack"),
        }
        assert!(matches!(
            serde_json::from_slice::<ConsumeUp>(br#"{"type":"subscribe","prefetch":4}"#).unwrap(),
            ConsumeUp::Subscribe { prefetch: 4, .. }
        ));
        assert!(matches!(
            serde_json::from_slice::<ConsumeUp>(br#"{"type":"nack","lease_id":"x"}"#).unwrap(),
            ConsumeUp::Nack { .. }
        ));
        let e = LeasedEntry {
            lease_id: "L".into(),
            epoch: 1,
            message_id: "m".into(),
            payload: serde_json::json!({"k": 1}),
        };
        assert!(encode_frame(&e).len() > 4);
    }

    #[test]
    fn inflight_normal_ack() {
        let mut inflight = InFlight::new();
        assert_eq!(inflight.count(), 0);
        assert!(inflight.is_empty());

        inflight.deliver("lease-1", 10);
        assert_eq!(inflight.count(), 1);
        assert!(!inflight.is_empty());
        assert_eq!(inflight.epoch("lease-1"), Some(10));

        // Normal ack frees a credit
        assert!(inflight.ack("lease-1"));
        assert_eq!(inflight.count(), 0);
        assert!(inflight.is_empty());
    }

    #[test]
    fn inflight_normal_nack() {
        let mut inflight = InFlight::new();
        inflight.deliver("lease-1", 42);
        assert_eq!(inflight.count(), 1);

        // Normal nack returns the epoch and frees a credit
        assert_eq!(inflight.nack("lease-1"), Some(42));
        assert_eq!(inflight.count(), 0);
    }

    #[test]
    fn inflight_unknown_id_ack_and_nack() {
        let mut inflight = InFlight::new();
        inflight.deliver("lease-1", 1);
        assert_eq!(inflight.count(), 1);

        // Unknown-id ack frees no credit
        assert!(!inflight.ack("unknown-lease"));
        assert_eq!(inflight.count(), 1);

        // Unknown-id nack returns None and frees no credit
        assert_eq!(inflight.nack("unknown-lease"), None);
        assert_eq!(inflight.count(), 1);

        // Original lease remains intact
        assert_eq!(inflight.epoch("lease-1"), Some(1));
    }

    #[test]
    fn inflight_duplicate_ack_and_nack() {
        let mut inflight = InFlight::new();
        inflight.deliver("lease-1", 1);
        inflight.deliver("lease-2", 2);
        assert_eq!(inflight.count(), 2);

        // First ack succeeds and frees credit
        assert!(inflight.ack("lease-1"));
        assert_eq!(inflight.count(), 1);

        // Duplicate ack frees no credit
        assert!(!inflight.ack("lease-1"));
        assert_eq!(inflight.count(), 1);

        // First nack succeeds and frees credit
        assert_eq!(inflight.nack("lease-2"), Some(2));
        assert_eq!(inflight.count(), 0);

        // Duplicate nack frees no credit
        assert_eq!(inflight.nack("lease-2"), None);
        assert_eq!(inflight.count(), 0);
    }

    #[test]
    fn inflight_prefetch_bound_across_deliveries_and_bogus_acks() {
        let prefetch = 3;
        let mut inflight = InFlight::new();

        // 1. Fill up to prefetch
        for i in 1..=prefetch {
            inflight.deliver(format!("lease-{i}"), i as u64);
        }
        assert_eq!(inflight.count(), prefetch);

        // 2. A sequence of bogus acks / nacks must not free credit or allow exceeding prefetch
        let bogus_attempts = ["fake-1", "fake-2", "lease-99", "nonexistent"];
        for bogus in bogus_attempts {
            assert!(!inflight.ack(bogus));
            assert_eq!(inflight.nack(bogus), None);
            assert!(inflight.count() <= prefetch);
        }
        assert_eq!(inflight.count(), prefetch);

        // 3. Normal ack opens one slot
        assert!(inflight.ack("lease-1"));
        assert_eq!(inflight.count(), prefetch - 1);

        // 4. Duplicate ack of already-acked lease must NOT open another slot
        assert!(!inflight.ack("lease-1"));
        assert_eq!(inflight.count(), prefetch - 1);

        // 5. Fill the single open slot with a new delivery
        inflight.deliver("lease-4", 4);
        assert_eq!(inflight.count(), prefetch);

        // 6. Normal nack opens one slot
        assert_eq!(inflight.nack("lease-2"), Some(2));
        assert_eq!(inflight.count(), prefetch - 1);

        // 7. Duplicate nack must NOT open another slot
        assert_eq!(inflight.nack("lease-2"), None);
        assert_eq!(inflight.count(), prefetch - 1);

        // 8. Deliver one more up to prefetch bound
        inflight.deliver("lease-5", 5);
        assert_eq!(inflight.count(), prefetch);

        // In-flight count never exceeds prefetch
        assert!(inflight.count() <= prefetch);

        // Drain all remaining valid leases
        assert!(inflight.ack("lease-3"));
        assert!(inflight.ack("lease-4"));
        assert!(inflight.ack("lease-5"));
        assert_eq!(inflight.count(), 0);
        assert!(inflight.is_empty());
    }
}
