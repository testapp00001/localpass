//! Adding a device with a short code.
//!
//! 1. SPAKE2 on the 8-digit code (an eavesdropper or impostor gets one online
//!    guess per attempt; nothing to brute-force offline), plus key confirmation.
//! 2. `Noise_XXpsk0` keyed by the SPAKE2 output: both static keys are proven.
//! 3. Inviter → `Offer{vault_id}`; joiner → `Request{keys, signature over the
//!    handshake hash}`; the inviter's user approves; inviter → `Accept{genesis,
//!    proof, VK, NK}`; joiner → `Done`.

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use lp_core::crypto::{self, Key32, NoiseKeypair, SigningKey, hkdf_sha256, hmac_sha256, sha256};
use lp_core::ids::{DeviceId, Sig64, VaultId};
use lp_core::model::{DeviceKeys, RosterOp};
use lp_core::op::{OpClass, SignedOp};
use lp_core::roster::{RosterRecord, RosterView};
use serde::{Deserialize, Serialize};
use spake2::{Ed25519Group, Identity, Password, Spake2};
use std::net::SocketAddr;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;
use zeroize::{Zeroize, Zeroizing};

use crate::channel::SecureChannel;
use crate::discovery::{Discovery, DiscoveryEvent, PAIR_SERVICE};
use crate::framing::{read_frame, write_frame};
use crate::netutil::is_local_ip;
use crate::protocol::PREAMBLE_PAIR;
use crate::{Result, SyncError};

pub const CODE_DIGITS: usize = 8;
pub const MAX_FAILED_ATTEMPTS: u32 = 3;
pub const INVITE_TTL: Duration = Duration::from_secs(300);
pub const APPROVAL_TIMEOUT: Duration = Duration::from_secs(180);
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

const PAIR_PROLOGUE: &[u8] = b"localpass/pair/1\0";
const JOIN_SIG_DOMAIN: &[u8] = b"lp/pair/join/v1\0";
const ID_JOINER: &[u8] = b"lp-joiner";
const ID_INVITER: &[u8] = b"lp-inviter";

pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// Random 8-digit code, e.g. `"40719325"`.
pub fn generate_code() -> Result<String> {
    let mut code = String::with_capacity(CODE_DIGITS);
    for _ in 0..CODE_DIGITS {
        code.push(char::from(b'0' + crypto::random_below(10)? as u8));
    }
    Ok(code)
}

/// Accepts "4071-9325", "4071 9325", … Returns `None` unless exactly 8 digits.
pub fn normalize_code(input: &str) -> Option<String> {
    let digits: String = input.chars().filter(|c| c.is_ascii_digit()).collect();
    (digits.len() == CODE_DIGITS
        && input
            .chars()
            .all(|c| c.is_ascii_digit() || c == '-' || c.is_whitespace()))
    .then_some(digits)
}

pub fn format_code(code: &str) -> String {
    if code.len() == CODE_DIGITS {
        format!("{}-{}", &code[..4], &code[4..])
    } else {
        code.to_string()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JoinRequest {
    pub keys: DeviceKeys,
    pub name: String,
    pub platform: String,
    /// Joiner's signature over the Noise handshake hash.
    pub session_sig: Sig64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct JoinAccept {
    pub vault_id: VaultId,
    pub genesis: SignedOp,
    /// Roster ops from genesis through the joiner's `DeviceAdded`.
    pub proof: Vec<SignedOp>,
    #[serde(with = "serde_bytes")]
    pub vk: Vec<u8>,
    #[serde(with = "serde_bytes")]
    pub nk: Vec<u8>,
    pub inviter: DeviceId,
    pub inviter_addrs: Vec<String>,
}

impl Drop for JoinAccept {
    fn drop(&mut self) {
        self.vk.zeroize();
        self.nk.zeroize();
    }
}

impl fmt::Debug for JoinAccept {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JoinAccept")
            .field("vault_id", &self.vault_id)
            .field("inviter", &self.inviter)
            .field("proof_len", &self.proof.len())
            .finish_non_exhaustive()
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "t")]
enum PairMsg {
    #[serde(rename = "offer")]
    Offer {
        vault_id: VaultId,
        inviter: DeviceId,
    },
    #[serde(rename = "request")]
    Request(JoinRequest),
    #[serde(rename = "accept")]
    Accept(Box<JoinAccept>),
    #[serde(rename = "reject")]
    Reject { reason: String },
    #[serde(rename = "done")]
    Done,
}

/// Engine hooks for the inviting side.
pub trait InviteHandler: Send + Sync {
    /// Ask the user. On approval, author `DeviceAdded` and return the payload;
    /// `Ok(None)` means the user declined.
    fn approve(
        &self,
        request: JoinRequest,
    ) -> BoxFuture<std::result::Result<Option<JoinAccept>, String>>;
}

/// Everything the inviter's listener needs for one pairing attempt.
pub struct InviterParams<'a> {
    pub code: &'a str,
    pub vault_id: VaultId,
    pub me: DeviceId,
    pub noise_secret: &'a Key32,
    pub handler: &'a dyn InviteHandler,
}

fn spake_password(code: &str) -> Password {
    Password::new(format!("lp-pair-v1:{code}").as_bytes())
}

fn confirm(key: &[u8], role: &[u8], transcript: &[u8]) -> [u8; 32] {
    hmac_sha256(key, &[b"lp/pair/confirm\0", role, transcript])
}

fn pair_prologue(transcript: &[u8]) -> Vec<u8> {
    let mut p = PAIR_PROLOGUE.to_vec();
    p.extend_from_slice(&sha256(transcript));
    p
}

/// Joining side. `stream` must already have sent the `LPP1` preamble.
pub async fn run_joiner<S>(
    mut stream: S,
    code: &str,
    sk: &SigningKey,
    noise: &NoiseKeypair,
    name: &str,
    platform: &str,
) -> Result<JoinAccept>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    // 1. SPAKE2 + confirmation.
    let (state, msg_a) = Spake2::<Ed25519Group>::start_a(
        &spake_password(code),
        &Identity::new(ID_JOINER),
        &Identity::new(ID_INVITER),
    );
    write_frame(&mut stream, &msg_a).await?;
    let mut msg_b = Vec::new();
    timeout(STEP_TIMEOUT, read_frame(&mut stream, &mut msg_b)).await??;
    let key = Zeroizing::new(
        state
            .finish(&msg_b)
            .map_err(|e| SyncError::Pairing(e.to_string()))?,
    );
    let transcript = [msg_a.as_slice(), msg_b.as_slice()].concat();
    write_frame(&mut stream, &confirm(&key, b"joiner", &transcript)).await?;
    let mut theirs = Vec::new();
    // The inviter closes the connection instead of confirming when the code is wrong.
    if timeout(STEP_TIMEOUT, read_frame(&mut stream, &mut theirs))
        .await
        .is_err()
        || !crypto::ct_eq(&theirs, &confirm(&key, b"inviter", &transcript))
    {
        return Err(SyncError::WrongCode);
    }

    // 2. Noise keyed by the SPAKE2 output.
    let psk = hkdf_sha256(&key[..], &transcript, b"lp/pair/psk/v1");
    let mut ch =
        SecureChannel::initiate(stream, noise.secret(), &psk, &pair_prologue(&transcript)).await?;

    // 3. Offer → Request → Accept → Done.
    let (vault_id, inviter) = match ch.recv().await? {
        PairMsg::Offer { vault_id, inviter } => (vault_id, inviter),
        _ => return Err(SyncError::Protocol("expected offer".into())),
    };
    let request = JoinRequest {
        keys: DeviceKeys::new(sk, &vault_id, noise.public()),
        name: name.to_string(),
        platform: platform.to_string(),
        session_sig: sk.sign(JOIN_SIG_DOMAIN, ch.handshake_hash()),
    };
    ch.send(&PairMsg::Request(request)).await?;
    let accept = match ch.recv_within(APPROVAL_TIMEOUT + STEP_TIMEOUT).await? {
        PairMsg::Accept(a) => *a,
        PairMsg::Reject { reason } => {
            return Err(SyncError::Pairing(format!(
                "the other device declined: {reason}"
            )));
        }
        _ => return Err(SyncError::Protocol("expected accept".into())),
    };
    validate_accept(&accept, vault_id, inviter, &sk.device_id())?;
    ch.send(&PairMsg::Done).await?;
    Ok(accept)
}

/// Connects to the first reachable inviter address and runs the joining side.
pub async fn join_via(
    addrs: &[SocketAddr],
    code: &str,
    sk: &SigningKey,
    noise: &NoiseKeypair,
    name: &str,
    platform: &str,
) -> Result<JoinAccept> {
    let mut last = SyncError::Pairing("no address for the other device".into());
    for addr in addrs {
        // The invite comes from a QR code or pasted text, which anyone can write: like every other
        // connection LocalPass makes, this one stays on the local network.
        if !is_local_ip(&addr.ip()) {
            last = SyncError::Pairing("that address is not on the local network".into());
            continue;
        }
        match timeout(Duration::from_secs(3), TcpStream::connect(addr)).await {
            Ok(Ok(mut stream)) => {
                let _ = stream.set_nodelay(true);
                stream.write_all(PREAMBLE_PAIR).await?;
                return run_joiner(stream, code, sk, noise, name, platform).await;
            }
            Ok(Err(e)) => last = e.into(),
            Err(_) => last = SyncError::Timeout,
        }
    }
    Err(last)
}

/// Looks for open invites on the local network. Returns shortly after the
/// first one is found, or empty after `wait`.
pub async fn discover_inviters(wait: Duration) -> Result<Vec<SocketAddr>> {
    let discovery = Discovery::start()?;
    let mut rx = discovery.browse(PAIR_SERVICE)?;
    let mut found: Vec<SocketAddr> = Vec::new();
    let mut deadline = tokio::time::Instant::now() + wait;
    loop {
        match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(DiscoveryEvent::Found(f))) => {
                for a in f.addrs {
                    if !found.contains(&a) {
                        found.push(a);
                    }
                }
                if !found.is_empty() {
                    deadline =
                        deadline.min(tokio::time::Instant::now() + Duration::from_millis(500));
                }
            }
            Ok(Some(DiscoveryEvent::Removed(_))) => {}
            Ok(None) | Err(_) => break,
        }
    }
    Ok(found)
}

fn validate_accept(
    a: &JoinAccept,
    vault_id: VaultId,
    inviter: DeviceId,
    me: &DeviceId,
) -> Result<()> {
    if a.vault_id != vault_id || a.inviter != inviter || a.vk.len() != 32 || a.nk.len() != 32 {
        return Err(SyncError::Pairing("inconsistent join data".into()));
    }
    let g = &a.genesis;
    if g.seq() != 1 || g.class() != OpClass::Roster || g.header().vault_id != vault_id {
        return Err(SyncError::Pairing("bad genesis op".into()));
    }
    let mut records = Vec::new();
    for op in std::iter::once(g).chain(a.proof.iter()) {
        if op.header().vault_id != vault_id || op.class() != OpClass::Roster {
            return Err(SyncError::Pairing("bad proof op".into()));
        }
        op.verify_signature()
            .map_err(|_| SyncError::Pairing("bad proof signature".into()))?;
        let decoded = op
            .decode_roster()
            .map_err(|_| SyncError::Pairing("bad proof op".into()))?;
        records.push(RosterRecord {
            author: op.author(),
            seq: op.seq(),
            op: decoded,
        });
    }
    match &records[0].op {
        RosterOp::Genesis {
            vault_id: v, keys, ..
        } if *v == vault_id && keys.sign_pk == g.author() => {}
        _ => return Err(SyncError::Pairing("bad genesis op".into())),
    }
    let view = RosterView::compute(vault_id, g.author(), &records)
        .ok_or_else(|| SyncError::Pairing("bad genesis op".into()))?;
    if !view.is_active(me) || !view.is_active(&inviter) {
        return Err(SyncError::Pairing(
            "proof does not admit this device".into(),
        ));
    }
    Ok(())
}

/// Why an inviter-side attempt ended without a new device.
#[derive(Debug)]
pub enum InviteFailure {
    WrongCode,
    Declined,
    Other(SyncError),
}

impl From<SyncError> for InviteFailure {
    fn from(e: SyncError) -> Self {
        InviteFailure::Other(e)
    }
}

impl From<std::io::Error> for InviteFailure {
    fn from(e: std::io::Error) -> Self {
        InviteFailure::Other(e.into())
    }
}

/// Inviting side. `stream` has already delivered the `LPP1` preamble.
pub async fn run_inviter<S>(
    mut stream: S,
    p: &InviterParams<'_>,
) -> std::result::Result<DeviceId, InviteFailure>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut msg_a = Vec::new();
    timeout(STEP_TIMEOUT, read_frame(&mut stream, &mut msg_a))
        .await
        .map_err(SyncError::from)??;
    let (state, msg_b) = Spake2::<Ed25519Group>::start_b(
        &spake_password(p.code),
        &Identity::new(ID_JOINER),
        &Identity::new(ID_INVITER),
    );
    write_frame(&mut stream, &msg_b)
        .await
        .map_err(SyncError::from)?;
    let key = Zeroizing::new(
        state
            .finish(&msg_a)
            .map_err(|e| SyncError::Pairing(e.to_string()))?,
    );
    let transcript = [msg_a.as_slice(), msg_b.as_slice()].concat();
    let mut theirs = Vec::new();
    timeout(STEP_TIMEOUT, read_frame(&mut stream, &mut theirs))
        .await
        .map_err(SyncError::from)??;
    if !crypto::ct_eq(&theirs, &confirm(&key, b"joiner", &transcript)) {
        return Err(InviteFailure::WrongCode);
    }
    write_frame(&mut stream, &confirm(&key, b"inviter", &transcript))
        .await
        .map_err(SyncError::from)?;

    let psk = hkdf_sha256(&key[..], &transcript, b"lp/pair/psk/v1");
    let mut ch =
        SecureChannel::respond(stream, p.noise_secret, &psk, &pair_prologue(&transcript)).await?;
    ch.send(&PairMsg::Offer {
        vault_id: p.vault_id,
        inviter: p.me,
    })
    .await?;
    let request = match ch.recv().await? {
        PairMsg::Request(r) => r,
        _ => return Err(SyncError::Protocol("expected join request".into()).into()),
    };
    let keys_ok = request.keys.noise_pk == ch.remote_static()
        && request.keys.verify_binding(&p.vault_id).is_ok()
        && crypto::verify(
            &request.keys.sign_pk,
            JOIN_SIG_DOMAIN,
            ch.handshake_hash(),
            &request.session_sig,
        )
        .is_ok();
    if !keys_ok {
        let _ = ch
            .send(&PairMsg::Reject {
                reason: "invalid device keys".into(),
            })
            .await;
        return Err(SyncError::Pairing("joiner sent invalid keys".into()).into());
    }
    let joiner = request.keys.sign_pk;
    let decision = timeout(APPROVAL_TIMEOUT, p.handler.approve(request)).await;
    let accept = match decision {
        Ok(Ok(Some(accept))) => accept,
        Ok(Ok(None)) | Err(_) => {
            let _ = ch
                .send(&PairMsg::Reject {
                    reason: "not approved".into(),
                })
                .await;
            return Err(InviteFailure::Declined);
        }
        Ok(Err(e)) => {
            let _ = ch.send(&PairMsg::Reject { reason: e.clone() }).await;
            return Err(SyncError::Pairing(e).into());
        }
    };
    ch.send(&PairMsg::Accept(Box::new(accept))).await?;
    match ch.recv().await? {
        PairMsg::Done => Ok(joiner),
        _ => Err(SyncError::Protocol("expected done".into()).into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lp_core::op::create_roster_op;
    use std::sync::{Arc, Mutex};

    /// RB-9: a hostile QR code cannot make the joining device dial the internet.
    #[tokio::test]
    async fn joining_never_dials_an_address_outside_the_local_network() {
        let sk = SigningKey::generate().unwrap();
        let noise = NoiseKeypair::generate().unwrap();
        let addrs: Vec<SocketAddr> = vec![
            "203.0.113.5:47821".parse().unwrap(),
            "[2001:db8::1]:47821".parse().unwrap(),
        ];
        let start = std::time::Instant::now();
        let err = join_via(&addrs, "12345678", &sk, &noise, "Phone", "android")
            .await
            .unwrap_err();
        assert!(
            matches!(&err, SyncError::Pairing(m) if m.contains("local network")),
            "{err:?}"
        );
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "refused without dialing"
        );
    }

    #[test]
    fn codes() {
        let c = generate_code().unwrap();
        assert_eq!(c.len(), 8);
        assert_eq!(normalize_code(&format_code(&c)).unwrap(), c);
        assert_eq!(normalize_code(" 1234 5678 ").as_deref(), Some("12345678"));
        assert!(normalize_code("1234-567").is_none());
        assert!(normalize_code("1234-567x8").is_none());
    }

    struct Inviter {
        vault: VaultId,
        sk: SigningKey,
        noise: NoiseKeypair,
        genesis: SignedOp,
        seen: Mutex<Option<JoinRequest>>,
        approve: bool,
    }

    impl InviteHandler for Inviter {
        fn approve(
            &self,
            request: JoinRequest,
        ) -> BoxFuture<std::result::Result<Option<JoinAccept>, String>> {
            *self.seen.lock().unwrap() = Some(request.clone());
            if !self.approve {
                return Box::pin(async { Ok(None) });
            }
            let added = create_roster_op(
                self.vault,
                &self.sk,
                2,
                &RosterOp::DeviceAdded {
                    keys: request.keys.clone(),
                    added_ms: 0,
                },
            )
            .unwrap();
            let accept = JoinAccept {
                vault_id: self.vault,
                genesis: self.genesis.clone(),
                proof: vec![added],
                vk: vec![1; 32],
                nk: vec![2; 32],
                inviter: self.sk.device_id(),
                inviter_addrs: vec![],
            };
            Box::pin(async move { Ok(Some(accept)) })
        }
    }

    fn inviter(approve: bool) -> Arc<Inviter> {
        let vault = VaultId::random().unwrap();
        let sk = SigningKey::generate().unwrap();
        let noise = NoiseKeypair::generate().unwrap();
        let keys = DeviceKeys::new(&sk, &vault, noise.public());
        let genesis = create_roster_op(
            vault,
            &sk,
            1,
            &RosterOp::Genesis {
                vault_id: vault,
                keys,
                created_ms: 0,
            },
        )
        .unwrap();
        Arc::new(Inviter {
            vault,
            sk,
            noise,
            genesis,
            seen: Mutex::new(None),
            approve,
        })
    }

    async fn attempt(
        inv: Arc<Inviter>,
        inviter_code: &'static str,
        joiner_code: &'static str,
    ) -> (
        std::result::Result<DeviceId, InviteFailure>,
        Result<JoinAccept>,
        DeviceId,
    ) {
        let (a, b) = tokio::io::duplex(1 << 16);
        let inv2 = inv.clone();
        let t = tokio::spawn(async move {
            let p = InviterParams {
                code: inviter_code,
                vault_id: inv2.vault,
                me: inv2.sk.device_id(),
                noise_secret: inv2.noise.secret(),
                handler: inv2.as_ref(),
            };
            run_inviter(b, &p).await
        });
        let sk = SigningKey::generate().unwrap();
        let noise = NoiseKeypair::generate().unwrap();
        let joined = run_joiner(a, joiner_code, &sk, &noise, "Work Laptop", "windows").await;
        (t.await.unwrap(), joined, sk.device_id())
    }

    #[tokio::test]
    async fn pairing_with_correct_code_admits_the_device() {
        let inv = inviter(true);
        let (inviter_res, joiner_res, joiner_id) =
            attempt(inv.clone(), "12345678", "12345678").await;
        assert_eq!(inviter_res.unwrap(), joiner_id);
        let accept = joiner_res.unwrap();
        assert_eq!(accept.vault_id, inv.vault);
        assert_eq!(accept.vk, vec![1; 32]);
        let seen = inv.seen.lock().unwrap().clone().unwrap();
        assert_eq!(seen.name, "Work Laptop");
        assert_eq!(seen.keys.sign_pk, joiner_id);
    }

    #[tokio::test]
    async fn wrong_code_is_detected_on_both_sides() {
        let inv = inviter(true);
        let (inviter_res, joiner_res, _) = attempt(inv.clone(), "12345678", "87654321").await;
        assert!(matches!(inviter_res, Err(InviteFailure::WrongCode)));
        assert!(matches!(joiner_res, Err(SyncError::WrongCode)));
        assert!(
            inv.seen.lock().unwrap().is_none(),
            "no request may reach the user"
        );
    }

    #[tokio::test]
    async fn declined_request_is_reported_to_joiner() {
        let inv = inviter(false);
        let (inviter_res, joiner_res, _) = attempt(inv, "11112222", "11112222").await;
        assert!(matches!(inviter_res, Err(InviteFailure::Declined)));
        assert!(matches!(joiner_res, Err(SyncError::Pairing(_))));
    }
}
