//! What browsers, the Android credential services and the Windows passkey provider ask of the
//! vault: logins for a website, saving a login, and creating and using passkeys.
//!
//! The origin always comes from the caller's trusted side (the browser, the OS), and it is
//! checked again here, so a confused or hostile front end can't get credentials for another
//! site. Passwords and private keys are handled in this module and only leave it when a
//! request needs them: signing happens here, so a passkey's private key never leaves the vault.

use lp_core::hlc::now_ms;
use lp_core::ids::ItemId;
use lp_core::materialize::ItemState;
use lp_core::model::{DataOp, FieldSet, FieldValue, keys as fkeys, kinds};
use lp_core::origin::{self, MatchKind, Origin};
use lp_core::passkey::{self, PasskeyKey, b64u};
pub use lp_core::passkey::{
    PasskeyAssertRequest, PasskeyAssertion, PasskeyCandidate, PasskeyCreateRequest, PasskeyCreated,
};
use lp_core::totp::TotpSpec;
use serde::Serialize;
use zeroize::Zeroizing;

use crate::items::{self, ItemInput, ItemQuery, ItemSummary, ItemView};
use crate::{Engine, EngineError, Result};

/// A saved login that fits the page (no secrets).
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginMatch {
    pub id: String,
    pub title: String,
    pub username: String,
    pub url: Option<String>,
    /// Same host as the page, as opposed to the same registrable domain.
    pub exact: bool,
    pub has_password: bool,
    pub has_totp: bool,
    pub favorite: bool,
}

/// What to type into a login form.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginFill {
    pub id: String,
    pub username: String,
    pub password: String,
    /// The current 2FA code, if the login has one.
    pub totp_code: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SaveStatus {
    Created,
    Updated,
    Unchanged,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveLoginOutcome {
    pub id: String,
    pub status: SaveStatus,
}

/// An existing login whose password a save would change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveChange {
    pub id: String,
    /// The title the user gave it.
    pub title: String,
    pub username: String,
    /// The first address it is saved for.
    pub saved_url: Option<String>,
    /// The save comes from the very host the login is saved for; otherwise from another address
    /// of the same site (`login.example.com` for a login saved for `example.com`).
    pub same_host: bool,
}

/// What saving a login would do (see [`Engine::plan_save_login`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase", tag = "plan")]
pub enum SavePlan {
    /// A new login would be made.
    Create,
    /// The same login with the same password is saved already.
    Unchanged { id: String },
    /// An existing login's password would change: ask the user first.
    Update(SaveChange),
}

/// Longest user-facing text accepted in a passkey request (characters).
const MAX_TEXT_CHARS: usize = 256;
/// Most credential ids in an exclude or allow list.
pub(crate) const MAX_ID_LIST: usize = 256;
/// Longest credential id (WebAuthn's own limit).
const MAX_CREDENTIAL_ID: usize = 1023;

fn check_text(what: &str, text: &str) -> Result<()> {
    if text.chars().count() > MAX_TEXT_CHARS {
        return Err(EngineError::Invalid(format!(
            "the passkey request has a too long {what} (over {MAX_TEXT_CHARS} characters)"
        )));
    }
    Ok(())
}

fn check_id_list(what: &str, ids: &[Vec<u8>]) -> Result<()> {
    if ids.len() > MAX_ID_LIST || ids.iter().any(|id| id.len() > MAX_CREDENTIAL_ID) {
        return Err(EngineError::Invalid(format!(
            "the passkey request has a too long {what} list"
        )));
    }
    Ok(())
}

/// The non-secret fields of a passkey item.
struct PasskeyRecord {
    rp_id: String,
    rp_name: String,
    user_name: String,
    user_display_name: String,
    credential_id: Vec<u8>,
    user_handle: Vec<u8>,
    created_ms: u64,
}

fn passkey_record(item: &ItemState) -> Option<PasskeyRecord> {
    if item.text(fkeys::KIND) != Some(kinds::PASSKEY) {
        return None;
    }
    let text = |k: &str| item.text(k).unwrap_or("").to_string();
    Some(PasskeyRecord {
        rp_id: item.text(fkeys::PASSKEY_RP_ID)?.to_string(),
        rp_name: text(fkeys::PASSKEY_RP_NAME),
        user_name: text(fkeys::PASSKEY_USER_NAME),
        user_display_name: text(fkeys::PASSKEY_USER_DISPLAY_NAME),
        credential_id: b64u::decode(item.text(fkeys::PASSKEY_CREDENTIAL_ID)?).ok()?,
        user_handle: b64u::decode(item.text(fkeys::PASSKEY_USER_HANDLE)?).ok()?,
        created_ms: item
            .text(fkeys::PASSKEY_CREATED_MS)
            .and_then(|t| t.parse().ok())
            .unwrap_or(0),
    })
}

fn candidate(item: &ItemState, record: PasskeyRecord) -> PasskeyCandidate {
    PasskeyCandidate {
        item_id: item.id.to_string(),
        rp_id: record.rp_id,
        rp_name: record.rp_name,
        user_name: record.user_name,
        user_display_name: record.user_display_name,
        credential_id: record.credential_id,
        user_handle: record.user_handle,
        created_ms: record.created_ms,
    }
}

fn input_from_view(v: &ItemView) -> ItemInput {
    ItemInput {
        id: Some(v.id.clone()),
        kind: v.kind.clone(),
        title: v.title.clone(),
        username: v.username.clone(),
        password: v.password.clone(),
        urls: v.urls.clone(),
        notes: v.notes.clone(),
        totp: v.totp.clone(),
        folder: v.folder.clone(),
        favorite: v.favorite,
        card: v.card.clone(),
        custom: v.custom.clone(),
    }
}

fn saved_urls(item: &ItemState) -> Vec<String> {
    item.get(fkeys::URLS)
        .and_then(FieldValue::as_list)
        .map(<[String]>::to_vec)
        .unwrap_or_default()
}

fn is_live_login(item: &ItemState) -> bool {
    !item.is_trashed() && item.text(fkeys::KIND).unwrap_or(kinds::LOGIN) == kinds::LOGIN
}

impl Engine {
    /// Logins that belong to the page at `origin`, best matches first. Contains no secrets.
    pub fn find_logins(&self, origin: &str) -> Result<Vec<LoginMatch>> {
        let origin = Origin::parse(origin)?;
        self.with_session(|s, _| {
            let mut found: Vec<(MatchKind, LoginMatch)> = s
                .state
                .items
                .values()
                .filter(|i| is_live_login(i))
                .filter(|i| i.text(fkeys::PASSWORD).is_some() || i.text(fkeys::USERNAME).is_some())
                .filter_map(|i| {
                    let urls = saved_urls(i);
                    let kind = origin::best_match(&urls, &origin)?;
                    Some((
                        kind,
                        LoginMatch {
                            id: i.id.to_string(),
                            title: i.text(fkeys::TITLE).unwrap_or("(untitled)").to_string(),
                            username: i.text(fkeys::USERNAME).unwrap_or("").to_string(),
                            url: urls.first().cloned(),
                            exact: kind == MatchKind::Exact,
                            has_password: i.text(fkeys::PASSWORD).is_some(),
                            has_totp: i.text(fkeys::TOTP).is_some(),
                            favorite: i.flag(fkeys::FAVORITE),
                        },
                    ))
                })
                .collect();
            found.sort_by_key(|(kind, m)| {
                (
                    std::cmp::Reverse(*kind),
                    !m.favorite,
                    m.title.to_lowercase(),
                )
            });
            Ok(found.into_iter().map(|(_, m)| m).collect())
        })
    }

    /// The username, password and current 2FA code to type into the page at `origin`.
    /// Refuses if the login doesn't belong to that site.
    pub fn fill_login(&self, id: &str, origin: &str) -> Result<LoginFill> {
        self.fill_login_checked(id, origin, true)
    }

    /// Like [`Engine::fill_login`] for a login saved for a different site (the user picked it
    /// from the full list). It still needs a well-formed secure origin and a live login: a
    /// trashed item, a note or a passkey is never filled. Callers must have asked the user first.
    pub fn fill_login_anywhere(&self, id: &str, origin: &str) -> Result<LoginFill> {
        self.fill_login_checked(id, origin, false)
    }

    fn fill_login_checked(&self, id: &str, origin: &str, same_site: bool) -> Result<LoginFill> {
        let origin = Origin::parse(origin)?;
        let id = ItemId::parse(id)?;
        self.with_session(|s, _| {
            let item = s
                .state
                .items
                .get(&id)
                .filter(|i| is_live_login(i))
                .ok_or_else(|| EngineError::NotFound("login".into()))?;
            if same_site && origin::best_match(&saved_urls(item), &origin).is_none() {
                return Err(EngineError::Invalid(
                    "this login is not saved for this website".into(),
                ));
            }
            let totp_code = match item.text(fkeys::TOTP) {
                Some(uri) => Some(TotpSpec::parse(uri)?.code_at(now_ms() / 1000)),
                None => None,
            };
            Ok(LoginFill {
                id: id.to_string(),
                username: item.text(fkeys::USERNAME).unwrap_or("").to_string(),
                password: item.text(fkeys::PASSWORD).unwrap_or("").to_string(),
                totp_code,
            })
        })
    }

    /// What a login is called (no secrets), if `id` is a live login. Callers use it to tell the
    /// user which login is about to be revealed, and to refuse anything else before asking.
    pub fn login_label(&self, id: &str) -> Result<LoginMatch> {
        let id = ItemId::parse(id)?;
        self.with_session(|s, _| {
            let item = s
                .state
                .items
                .get(&id)
                .filter(|i| is_live_login(i))
                .ok_or_else(|| EngineError::NotFound("login".into()))?;
            let urls = saved_urls(item);
            Ok(LoginMatch {
                id: item.id.to_string(),
                title: item.text(fkeys::TITLE).unwrap_or("(untitled)").to_string(),
                username: item.text(fkeys::USERNAME).unwrap_or("").to_string(),
                url: urls.first().cloned(),
                exact: false,
                has_password: item.text(fkeys::PASSWORD).is_some(),
                has_totp: item.text(fkeys::TOTP).is_some(),
                favorite: item.flag(fkeys::FAVORITE),
            })
        })
    }

    /// Live logins whose title, username, notes or address contain `query`, favourites first,
    /// at most `limit`. No secrets. This reaches logins saved for any site, so a caller must
    /// ask the user before it answers a browser.
    pub fn search_logins(&self, query: &str, limit: usize) -> Result<Vec<ItemSummary>> {
        let query = ItemQuery {
            search: Some(query.to_string()),
            ..Default::default()
        };
        self.with_session(|s, _| {
            let mut found: Vec<ItemSummary> = s
                .state
                .items
                .values()
                .filter(|i| is_live_login(i) && items::matches(i, &query))
                .map(items::summary)
                .collect();
            found.sort_by_key(|i| (!i.favorite, i.title.to_lowercase()));
            found.truncate(limit);
            Ok(found)
        })
    }

    /// What saving this login would do, without doing it: create a new login, find the same
    /// login already saved, or change the password of an existing login. The last one is what a
    /// caller must ask the user about first (see [`SavePlan::Update`]).
    ///
    /// The existing login is the best match for this user name over the saved addresses: the
    /// same host first, then another host of the same registrable domain.
    pub fn plan_save_login(
        &self,
        origin: &str,
        username: &str,
        password: &str,
    ) -> Result<SavePlan> {
        if password.is_empty() {
            return Err(EngineError::Invalid("there is no password to save".into()));
        }
        let origin = Origin::parse(origin)?;
        let username = username.trim();
        let found = self.with_session(|s, _| {
            Ok(s.state
                .items
                .values()
                .filter(|i| is_live_login(i))
                .filter(|i| {
                    i.text(fkeys::USERNAME)
                        .unwrap_or("")
                        .trim()
                        .eq_ignore_ascii_case(username)
                })
                .filter_map(|i| {
                    origin::best_match(&saved_urls(i), &origin).map(|kind| {
                        (
                            kind,
                            SaveChange {
                                id: i.id.to_string(),
                                title: i.text(fkeys::TITLE).unwrap_or("(untitled)").to_string(),
                                username: i.text(fkeys::USERNAME).unwrap_or("").to_string(),
                                saved_url: saved_urls(i).first().cloned(),
                                same_host: kind == MatchKind::Exact,
                            },
                            i.text(fkeys::PASSWORD).unwrap_or("") == password,
                        )
                    })
                })
                .max_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id))))
        })?;
        Ok(match found {
            None => SavePlan::Create,
            Some((_, change, true)) => SavePlan::Unchanged { id: change.id },
            Some((_, change, false)) => SavePlan::Update(change),
        })
    }

    /// Saves a login captured from a page without asking anybody: creates a new login, or
    /// updates the password of the login saved for the very same host and user name. A login
    /// saved for another address of the site (`example.com` for a save from `login.example.com`,
    /// or the reverse) is never touched: this makes a login of its own. Callers that can ask
    /// the user use [`Engine::plan_save_login`] first, to offer that update instead.
    pub fn save_login(
        &self,
        origin: &str,
        username: &str,
        password: &str,
        title: Option<&str>,
    ) -> Result<SaveLoginOutcome> {
        match self.plan_save_login(origin, username, password)? {
            SavePlan::Unchanged { id } => Ok(SaveLoginOutcome {
                id,
                status: SaveStatus::Unchanged,
            }),
            SavePlan::Update(change) if change.same_host => {
                self.update_login_password(&change.id, password)
            }
            SavePlan::Update(_) | SavePlan::Create => {
                self.create_login(origin, username, password, title)
            }
        }
    }

    /// Makes a new login for `origin` (never changes an existing one).
    pub fn create_login(
        &self,
        origin: &str,
        username: &str,
        password: &str,
        title: Option<&str>,
    ) -> Result<SaveLoginOutcome> {
        if password.is_empty() {
            return Err(EngineError::Invalid("there is no password to save".into()));
        }
        let origin = Origin::parse(origin)?;
        let name = title
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(str::to_string)
            .or_else(|| origin.registrable_domain())
            .unwrap_or_else(|| origin.host.clone());
        let id = self.save_item(&ItemInput {
            title: name,
            username: username.trim().to_string(),
            password: password.to_string(),
            urls: vec![origin.serialized()],
            ..Default::default()
        })?;
        Ok(SaveLoginOutcome {
            id,
            status: SaveStatus::Created,
        })
    }

    /// Changes the password of a live login, keeping everything else (the old one goes to the
    /// history). This is the step after the user agreed to a [`SavePlan::Update`].
    pub fn update_login_password(&self, id: &str, password: &str) -> Result<SaveLoginOutcome> {
        if password.is_empty() {
            return Err(EngineError::Invalid("there is no password to save".into()));
        }
        let item_id = ItemId::parse(id)?;
        self.with_session(|s, _| {
            s.state
                .items
                .get(&item_id)
                .filter(|i| is_live_login(i))
                .map(|_| ())
                .ok_or_else(|| EngineError::NotFound("login".into()))
        })?;
        let view = self.get_item(id)?;
        if view.password == password {
            return Ok(SaveLoginOutcome {
                id: id.to_string(),
                status: SaveStatus::Unchanged,
            });
        }
        let mut input = input_from_view(&view);
        input.password = password.to_string();
        self.save_item(&input)?;
        Ok(SaveLoginOutcome {
            id: id.to_string(),
            status: SaveStatus::Updated,
        })
    }

    /// Passkeys for `rp_id`, limited to `allow` credential ids when that list is not empty.
    pub fn passkey_candidates(
        &self,
        origin: &str,
        rp_id: &str,
        allow: &[Vec<u8>],
    ) -> Result<Vec<PasskeyCandidate>> {
        let origin = Origin::parse(origin)?;
        origin::validate_rp_id(rp_id, &origin)?;
        check_id_list("allow", allow)?;
        self.with_session(|s, _| {
            let mut found: Vec<PasskeyCandidate> = s
                .state
                .items
                .values()
                .filter(|i| !i.is_trashed())
                .filter_map(|i| passkey_record(i).map(|r| (i, r)))
                .filter(|(_, r)| r.rp_id == rp_id)
                .filter(|(_, r)| allow.is_empty() || allow.contains(&r.credential_id))
                .map(|(i, r)| candidate(i, r))
                .collect();
            found.sort_by(|a, b| {
                a.user_name
                    .to_lowercase()
                    .cmp(&b.user_name.to_lowercase())
                    .then(b.created_ms.cmp(&a.created_ms))
            });
            Ok(found)
        })
    }

    /// Every live passkey in the vault, for the Windows passkey provider's credential cache.
    /// Metadata only: the private key never leaves the engine. Trashed passkeys are left out,
    /// like everywhere else. Ordered by site, then user name, newest first among equals.
    pub fn passkey_list_all(&self) -> Result<Vec<PasskeyCandidate>> {
        self.with_session(|s, _| {
            let mut found: Vec<PasskeyCandidate> = s
                .state
                .items
                .values()
                .filter(|i| !i.is_trashed())
                .filter_map(|i| passkey_record(i).map(|r| candidate(i, r)))
                .collect();
            found.sort_by(|a, b| {
                a.rp_id
                    .cmp(&b.rp_id)
                    .then(a.user_name.to_lowercase().cmp(&b.user_name.to_lowercase()))
                    .then(b.created_ms.cmp(&a.created_ms))
                    .then(a.item_id.cmp(&b.item_id))
            });
            Ok(found)
        })
    }

    /// Checks that `passkey_create` would accept the request, without changing anything. Callers
    /// use it to fail before they ask the user to confirm.
    pub fn passkey_can_create(&self, req: &PasskeyCreateRequest) -> Result<()> {
        let origin = Origin::parse(&req.origin)?;
        origin::validate_rp_id(&req.rp_id, &origin)?;
        if !req.algorithms.is_empty() && !req.algorithms.contains(&passkey::ALG_ES256) {
            return Err(EngineError::Unsupported(
                "this website needs a passkey type LocalPass doesn't support yet".into(),
            ));
        }
        if req.user_handle.is_empty() || req.user_handle.len() > 64 {
            return Err(EngineError::Invalid("invalid passkey user handle".into()));
        }
        check_text("site name", &req.rp_name)?;
        check_text("user name", &req.user_name)?;
        check_text("display name", &req.user_display_name)?;
        check_id_list("exclude", &req.exclude)?;
        self.with_session(|s, _| {
            let excluded = s
                .state
                .items
                .values()
                .filter(|i| !i.is_trashed())
                .filter_map(passkey_record)
                .any(|r| r.rp_id == req.rp_id && req.exclude.contains(&r.credential_id));
            if excluded {
                Err(EngineError::PasskeyExists)
            } else {
                Ok(())
            }
        })
    }

    /// The passkey that [`Engine::passkey_create`] would replace for this request (same site and
    /// user handle), if there is one. Changes nothing; fails like [`Engine::passkey_can_create`].
    /// Callers ask the user before creating over it.
    pub fn passkey_would_replace(
        &self,
        req: &PasskeyCreateRequest,
    ) -> Result<Option<PasskeyCandidate>> {
        self.passkey_can_create(req)?;
        self.with_session(|s, _| {
            Ok(s.state
                .items
                .values()
                .filter(|i| !i.is_trashed())
                .filter_map(|i| passkey_record(i).map(|r| (i, r)))
                // The last match, because that is the one `passkey_create` replaces.
                .rfind(|(_, r)| r.rp_id == req.rp_id && r.user_handle == req.user_handle)
                .map(|(i, r)| candidate(i, r)))
        })
    }

    /// Creates a passkey and stores it in the vault. A second registration for the same
    /// account (site and user handle) replaces the first, as the WebAuthn spec asks.
    pub fn passkey_create(&self, req: &PasskeyCreateRequest) -> Result<PasskeyCreated> {
        self.passkey_can_create(req)?;

        let key = PasskeyKey::generate()?;
        let public = key.public();
        let credential_id = passkey::new_credential_id()?;
        let mut flags = passkey::FLAG_UP | passkey::FLAG_BE | passkey::FLAG_BS;
        if req.user_verified {
            flags |= passkey::FLAG_UV;
        }
        let auth_data =
            passkey::authenticator_data(&req.rp_id, flags, 0, Some((&credential_id, &public)));
        let attestation_object = passkey::attestation_object_none(&auth_data);
        let private_key = Zeroizing::new(b64u::encode(&key.to_pkcs8_der()?));

        let item_id = self.with_session(|s, ctx| {
            let mut same_account = None;
            for item in s.state.items.values().filter(|i| !i.is_trashed()) {
                let Some(record) = passkey_record(item) else {
                    continue;
                };
                if record.rp_id != req.rp_id {
                    continue;
                }
                if req.exclude.contains(&record.credential_id) {
                    return Err(EngineError::PasskeyExists);
                }
                if record.user_handle == req.user_handle {
                    same_account = Some(item.id);
                }
            }
            let (id, existing) = match same_account {
                Some(id) => (id, s.state.items.get(&id)),
                None => (ItemId::random()?, None),
            };
            let set = |key: &str, value: FieldValue| FieldSet {
                key: key.to_string(),
                value,
                base: existing
                    .and_then(|i| i.winner(key))
                    .map(|w| w.stamp.op_ref()),
            };
            let text = |key: &str, value: &str| set(key, FieldValue::Text(value.to_string()));
            let mut fields = vec![
                text(fkeys::PASSKEY_RP_ID, &req.rp_id),
                text(fkeys::PASSKEY_CREDENTIAL_ID, &b64u::encode(&credential_id)),
                text(fkeys::PASSKEY_USER_HANDLE, &b64u::encode(&req.user_handle)),
                text(fkeys::PASSKEY_PRIVATE_KEY, &private_key),
                text(fkeys::PASSKEY_CREATED_MS, &now_ms().to_string()),
            ];
            for (key, value) in [
                (fkeys::PASSKEY_RP_NAME, req.rp_name.as_str()),
                (fkeys::PASSKEY_USER_NAME, req.user_name.as_str()),
                (
                    fkeys::PASSKEY_USER_DISPLAY_NAME,
                    req.user_display_name.as_str(),
                ),
            ] {
                if !value.is_empty() || existing.is_some() {
                    fields.push(if value.is_empty() {
                        set(key, FieldValue::Null)
                    } else {
                        text(key, value)
                    });
                }
            }
            if existing.is_none() {
                let title = if req.rp_name.trim().is_empty() {
                    req.rp_id.as_str()
                } else {
                    req.rp_name.trim()
                };
                fields.push(text(fkeys::KIND, kinds::PASSKEY));
                fields.push(text(fkeys::TITLE, title));
                fields.push(set(
                    fkeys::URLS,
                    FieldValue::List(vec![format!("https://{}", req.rp_id)]),
                ));
                if !req.user_name.is_empty() {
                    // Lists show the username under the title.
                    fields.push(text(fkeys::USERNAME, &req.user_name));
                }
            }
            let hlc = s.clock.tick(now_ms());
            Self::write_data(
                s,
                ctx,
                DataOp::ItemSet {
                    item: id,
                    hlc,
                    fields,
                },
            )?;
            Ok(id)
        })?;
        self.after_local_write();

        Ok(PasskeyCreated {
            item_id: item_id.to_string(),
            credential_id,
            auth_data,
            attestation_object,
            public_key: public.to_cose(),
            public_key_spki: public.to_spki_der(),
            algorithm: passkey::ALG_ES256,
        })
    }

    /// Signs a sign-in challenge with a stored passkey. The caller has already decided whether
    /// the user was verified and reports it in `user_verified`.
    pub fn passkey_assert(&self, req: &PasskeyAssertRequest) -> Result<PasskeyAssertion> {
        let origin = Origin::parse(&req.origin)?;
        origin::validate_rp_id(&req.rp_id, &origin)?;
        if req.client_data_hash.len() != 32 {
            return Err(EngineError::Invalid(
                "the client data hash must be 32 bytes".into(),
            ));
        }
        let id = ItemId::parse(&req.item_id)?;
        let mut flags = passkey::FLAG_UP | passkey::FLAG_BE | passkey::FLAG_BS;
        if req.user_verified {
            flags |= passkey::FLAG_UV;
        }
        self.with_session(|s, _| {
            let item = s
                .state
                .items
                .get(&id)
                .filter(|i| !i.is_trashed())
                .ok_or_else(|| EngineError::NotFound("passkey".into()))?;
            let record =
                passkey_record(item).ok_or_else(|| EngineError::NotFound("passkey".into()))?;
            if record.rp_id != req.rp_id {
                return Err(EngineError::Invalid(
                    "this passkey belongs to a different website".into(),
                ));
            }
            let private = item
                .text(fkeys::PASSKEY_PRIVATE_KEY)
                .and_then(|t| b64u::decode(t).ok())
                .map(Zeroizing::new)
                .ok_or_else(|| EngineError::Invalid("the passkey has no private key".into()))?;
            let key = PasskeyKey::from_pkcs8_der(&private)?;
            let auth_data = passkey::authenticator_data(&req.rp_id, flags, 0, None);
            let signature = key.assertion_signature(&auth_data, &req.client_data_hash);
            Ok(PasskeyAssertion {
                item_id: id.to_string(),
                credential_id: record.credential_id,
                auth_data,
                signature,
                user_handle: record.user_handle,
            })
        })
    }
}
