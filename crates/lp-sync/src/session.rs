//! One sync exchange between two vault members.
//!
//! ```text
//! initiator                         responder
//!   Hello ───────────────────────────►  verify peer
//!   verify peer  ◄─────────────────── Hello
//!   Ops* + Done (what B lacks) ──────►  ingest
//!   ingest      ◄─────────────────── Ops* + Done (what A lacks)
//! ```

use std::time::Duration;

use lp_core::crypto::Key32;
use lp_core::hlc::now_ms;
use lp_core::ids::{DeviceId, OpRef};
use lp_core::op::SignedOp;
use lp_store::VersionVector;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::time::timeout;

use crate::channel::SecureChannel;
use crate::protocol::{
    AddrHint, Hello, MAX_HINTS, MAX_OPS_PER_SESSION, PROTO_VERSION, SyncMsg, batches, sync_prologue,
};
use crate::replica::Replica;
use crate::{Result, SyncError};

pub const SESSION_TIMEOUT: Duration = Duration::from_secs(60);

pub struct SessionParams<'a> {
    pub replica: &'a Replica,
    pub noise_secret: &'a Key32,
    pub nk: &'a Key32,
    pub listen: Vec<String>,
    pub hints: Vec<AddrHint>,
}

#[derive(Debug, Default)]
pub struct SessionOutcome {
    pub peer: Option<DeviceId>,
    pub received: usize,
    pub sent: usize,
    pub roster_changed: bool,
    pub forks: Vec<OpRef>,
    pub peer_listen: Vec<String>,
    pub peer_hints: Vec<AddrHint>,
    /// Peer clock minus ours, in milliseconds.
    pub clock_skew_ms: i64,
}

pub async fn run_initiator<S>(stream: S, p: &SessionParams<'_>) -> Result<SessionOutcome>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    timeout(SESSION_TIMEOUT, initiator(stream, p)).await?
}

pub async fn run_responder<S>(stream: S, p: &SessionParams<'_>) -> Result<SessionOutcome>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    timeout(SESSION_TIMEOUT, responder(stream, p)).await?
}

async fn initiator<S>(stream: S, p: &SessionParams<'_>) -> Result<SessionOutcome>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let prologue = sync_prologue(&p.replica.vault_id());
    let mut ch = SecureChannel::initiate(stream, p.noise_secret, p.nk, &prologue).await?;
    ch.send(&SyncMsg::Hello(hello(p)?)).await?;
    let peer_hello = expect_hello(ch.recv().await?)?;
    let peer = match verify(p, ch.remote_static(), &peer_hello) {
        Ok(id) => id,
        Err(e) => {
            let _ = ch.send(&error_msg(&e)).await;
            return Err(e);
        }
    };
    let mut out = outcome(peer, &peer_hello);
    out.sent = send_ops(&mut ch, p.replica.ops_missing_for(&vv_of(&peer_hello))?).await?;
    receive_ops(&mut ch, p.replica, &mut out).await?;
    Ok(out)
}

async fn responder<S>(stream: S, p: &SessionParams<'_>) -> Result<SessionOutcome>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let prologue = sync_prologue(&p.replica.vault_id());
    let mut ch = SecureChannel::respond(stream, p.noise_secret, p.nk, &prologue).await?;
    let peer_hello = expect_hello(ch.recv().await?)?;
    let peer = match verify(p, ch.remote_static(), &peer_hello) {
        Ok(id) => id,
        Err(e) => {
            let _ = ch.send(&error_msg(&e)).await;
            return Err(e);
        }
    };
    ch.send(&SyncMsg::Hello(hello(p)?)).await?;
    let mut out = outcome(peer, &peer_hello);
    receive_ops(&mut ch, p.replica, &mut out).await?;
    // Ops just received have seq ≤ the initiator's vector, so they aren't echoed back.
    out.sent = send_ops(&mut ch, p.replica.ops_missing_for(&vv_of(&peer_hello))?).await?;
    Ok(out)
}

fn hello(p: &SessionParams<'_>) -> Result<Hello> {
    Ok(Hello {
        proto: PROTO_VERSION,
        now_ms: now_ms(),
        vv: p.replica.version_vector()?.into_iter().collect(),
        proof: p.replica.membership_proof()?,
        listen: p.listen.clone(),
        hints: p.hints.iter().take(MAX_HINTS).cloned().collect(),
    })
}

fn expect_hello(msg: SyncMsg) -> Result<Hello> {
    match msg {
        SyncMsg::Hello(h) => Ok(h),
        SyncMsg::Error { message, .. } => Err(SyncError::Remote(message)),
        _ => Err(SyncError::Protocol("expected hello".into())),
    }
}

fn verify(
    p: &SessionParams<'_>,
    remote: lp_core::ids::NoisePublicKey,
    h: &Hello,
) -> Result<DeviceId> {
    if h.proto != PROTO_VERSION {
        return Err(SyncError::Protocol(format!(
            "peer speaks protocol v{}, we speak v{PROTO_VERSION}; update the app on both devices",
            h.proto
        )));
    }
    let peer = p.replica.verify_peer(&remote, &h.proof)?;
    if peer == p.replica.me() {
        return Err(SyncError::Protocol("connected to ourselves".into()));
    }
    Ok(peer)
}

fn error_msg(e: &SyncError) -> SyncMsg {
    let code = match e {
        SyncError::NotMember => "not_member",
        SyncError::Revoked => "revoked",
        _ => "error",
    };
    SyncMsg::Error {
        code: code.into(),
        message: e.to_string(),
    }
}

fn vv_of(h: &Hello) -> VersionVector {
    h.vv.iter().copied().collect()
}

fn outcome(peer: DeviceId, h: &Hello) -> SessionOutcome {
    SessionOutcome {
        peer: Some(peer),
        peer_listen: h.listen.clone(),
        peer_hints: h.hints.iter().take(MAX_HINTS).cloned().collect(),
        clock_skew_ms: h.now_ms as i64 - now_ms() as i64,
        ..Default::default()
    }
}

async fn send_ops<S>(ch: &mut SecureChannel<S>, ops: Vec<SignedOp>) -> Result<usize>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let n = ops.len();
    for batch in batches(ops) {
        ch.send(&SyncMsg::Ops { ops: batch }).await?;
    }
    ch.send(&SyncMsg::Done).await?;
    Ok(n)
}

async fn receive_ops<S>(
    ch: &mut SecureChannel<S>,
    replica: &Replica,
    out: &mut SessionOutcome,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut staged = Vec::new();
    let mut total = 0usize;
    loop {
        match ch.recv::<SyncMsg>().await? {
            SyncMsg::Ops { ops } => {
                total += ops.len();
                if total > MAX_OPS_PER_SESSION {
                    return Err(SyncError::Protocol("peer sent too many ops".into()));
                }
                let report = replica.ingest(ops, &mut staged)?;
                out.received += report.accepted.len();
                out.roster_changed |= report.roster_changed;
                out.forks.extend(report.forks);
            }
            SyncMsg::Done => return Ok(()),
            SyncMsg::Error { message, .. } => return Err(SyncError::Remote(message)),
            SyncMsg::Hello(_) => return Err(SyncError::Protocol("unexpected hello".into())),
        }
    }
}
