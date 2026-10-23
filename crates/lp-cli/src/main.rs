//! `lp` — LocalPass from the command line. Useful as a headless sync peer and
//! for testing sync between machines before the GUI is involved.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use lp_core::passgen::PasswordOptions;
use lp_engine::{
    Engine, EngineConfig, EngineEvent, FileSecretStore, ItemInput, ItemQuery, VaultStatus,
};
use lp_sync::discovery::{Discovery, DiscoveryEvent, SYNC_SERVICE};
use lp_sync::netutil;
use lp_sync::service::{DEFAULT_PORT, SyncEvent};
use zeroize::Zeroizing;

#[derive(Parser)]
#[command(
    name = "lp",
    version,
    about = "LocalPass: a fully local password manager with LAN sync"
)]
struct Cli {
    /// Where this device's vault lives.
    #[arg(
        long,
        env = "LOCALPASS_DIR",
        default_value = ".localpass-dev/cli",
        global = true
    )]
    data_dir: PathBuf,
    /// TCP port for sync (falls back to a free port if taken).
    #[arg(long, env = "LOCALPASS_PORT", default_value_t = DEFAULT_PORT, global = true)]
    port: u16,
    /// Disable mDNS discovery (use --addr / peer-add instead).
    #[arg(long, global = true)]
    no_mdns: bool,
    /// Read the master password from this environment variable instead of prompting.
    #[arg(long, global = true)]
    password_env: Option<String>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new vault on this device.
    Init {
        #[arg(long)]
        name: String,
    },
    /// Join an existing vault with the code shown on another device.
    Join {
        /// The 8-digit code (or a localpass://pair?... QR payload).
        code: String,
        /// Address of the inviting device, if discovery can't find it.
        #[arg(long)]
        addr: Vec<String>,
        #[arg(long)]
        name: String,
    },
    /// Show vault, device and sync status.
    Status,
    /// List items.
    List {
        #[arg(long)]
        search: Option<String>,
        #[arg(long)]
        trash: bool,
    },
    /// Show one item, including secrets.
    Show { id: String },
    /// Add a login.
    Add {
        #[arg(long)]
        title: String,
        #[arg(long, default_value = "")]
        username: String,
        #[arg(long)]
        password: Option<String>,
        /// Generate a random password.
        #[arg(long)]
        generate: bool,
        #[arg(long)]
        url: Vec<String>,
        #[arg(long, default_value = "")]
        notes: String,
        /// otpauth:// URI or base32 secret.
        #[arg(long, default_value = "")]
        totp: String,
    },
    /// Change an item's password (or generate a new one).
    SetPassword {
        id: String,
        #[arg(long)]
        password: Option<String>,
        #[arg(long)]
        generate: bool,
    },
    /// Move an item to the trash (or delete it permanently with --purge).
    Rm {
        id: String,
        #[arg(long)]
        purge: bool,
    },
    /// Generate a password.
    Gen {
        #[arg(long, default_value_t = 20)]
        length: usize,
        #[arg(long)]
        no_symbols: bool,
    },
    /// Show the current 2FA code of an item.
    Totp { id: String },
    /// List devices in the vault.
    Devices,
    /// Revoke a lost device (by id from `devices`).
    Revoke { device: String },
    /// Open an invite for a new device and keep syncing until it joins.
    Invite {
        /// Approve the joining device without asking.
        #[arg(long)]
        yes: bool,
    },
    /// Run the background sync service until Ctrl+C.
    Serve,
    /// Sync once with every reachable device (or one address) and exit.
    Sync {
        #[arg(long)]
        addr: Option<String>,
    },
    /// Remember a device's address for networks without mDNS.
    PeerAdd {
        addr: String,
        /// Allow a non-private address.
        #[arg(long)]
        allow_public: bool,
    },
    /// Import a Bitwarden .json/.csv, browser .csv, KeePass .kdbx or .lpbackup file.
    Import {
        file: PathBuf,
        /// Env var holding the KeePass or backup password.
        #[arg(long)]
        file_password_env: Option<String>,
        /// KeePass key file.
        #[arg(long)]
        keyfile: Option<PathBuf>,
    },
    /// Export the vault: an encrypted backup (default) or plain Bitwarden JSON (--plain).
    Export {
        file: PathBuf,
        #[arg(long)]
        plain: bool,
        /// Env var holding the backup password.
        #[arg(long)]
        file_password_env: Option<String>,
    },
    /// Check what this network allows (interfaces, port, mDNS).
    Doctor,
    /// Delete this device's vault data (other devices keep theirs).
    Leave {
        #[arg(long)]
        yes: bool,
    },
}

fn password(cli: &Cli, prompt: &str) -> Result<Zeroizing<String>> {
    if let Some(var) = &cli.password_env {
        return Ok(Zeroizing::new(
            std::env::var(var).with_context(|| format!("${var} is not set"))?,
        ));
    }
    Ok(Zeroizing::new(rpassword::prompt_password(prompt)?))
}

fn new_password(cli: &Cli) -> Result<Zeroizing<String>> {
    let first = password(cli, "New master password: ")?;
    if cli.password_env.is_none() {
        let again = password(cli, "Repeat master password: ")?;
        if *first != *again {
            bail!("passwords don't match");
        }
    }
    Ok(first)
}

fn open(cli: &Cli, auto_approve: bool) -> Result<Engine> {
    let mut config = EngineConfig::new(&cli.data_dir, std::env::consts::OS);
    config.sync.port = cli.port;
    config.sync.mdns = !cli.no_mdns;
    config.auto_approve_joins = auto_approve;
    let secrets = Arc::new(FileSecretStore::new(cli.data_dir.join("device.key")));
    Engine::open(config, secrets).context("opening vault")
}

fn unlock(cli: &Cli, engine: &Engine) -> Result<()> {
    match engine.status() {
        VaultStatus::NoVault => bail!("no vault here yet: run `lp init` or `lp join`"),
        VaultStatus::Unlocked => Ok(()),
        VaultStatus::Locked => {
            let pw = password(cli, "Master password: ")?;
            engine.unlock(&pw).context("unlock")
        }
    }
}

fn generated(length: usize, symbols: bool) -> Result<String> {
    let opts = PasswordOptions {
        length,
        symbols,
        ..Default::default()
    };
    Ok(Engine::generate_password(&opts)?.to_string())
}

fn ago(ms: Option<u64>) -> String {
    match ms {
        None => "never".into(),
        Some(t) => {
            let secs = lp_core::hlc::now_ms().saturating_sub(t) / 1000;
            match secs {
                0..=59 => format!("{secs}s ago"),
                60..=3599 => format!("{}m ago", secs / 60),
                3600..=86399 => format!("{}h ago", secs / 3600),
                _ => format!("{}d ago", secs / 86400),
            }
        }
    }
}

fn print_event(ev: &EngineEvent) {
    match ev {
        EngineEvent::Sync(SyncEvent::Listening { port }) => println!("listening on port {port}"),
        EngineEvent::Sync(SyncEvent::PeerDiscovered { device, addrs }) => {
            println!(
                "found peer {} at {addrs:?}",
                device
                    .map(|d| d.fingerprint())
                    .unwrap_or_else(|| "(new device)".into())
            )
        }
        EngineEvent::Sync(SyncEvent::SyncCompleted {
            peer,
            addr,
            received,
            sent,
            clock_skew_ms,
            ..
        }) => {
            println!(
                "synced with {} ({addr}): received {received}, sent {sent}",
                peer.fingerprint()
            );
            if clock_skew_ms.abs() > lp_sync::service::SKEW_WARN_MS {
                println!(
                    "  warning: that device's clock is {}s off",
                    clock_skew_ms / 1000
                );
            }
        }
        EngineEvent::Sync(SyncEvent::SyncFailed { addr, error, .. }) => {
            println!("sync with {addr} failed: {error}")
        }
        EngineEvent::Sync(SyncEvent::NetworkChanged { ips }) => {
            println!("network changed: {ips:?}")
        }
        EngineEvent::Sync(SyncEvent::ForkDetected { op, .. }) => {
            println!("WARNING: forked log detected at {op:?}")
        }
        EngineEvent::Sync(SyncEvent::InviteWrongCode { attempts_left }) => {
            println!("someone entered a wrong code ({attempts_left} attempts left)")
        }
        EngineEvent::PairingFinished { device, error } => match (device, error) {
            (Some(d), _) => println!("device {} joined", &d[..8]),
            (_, Some(e)) => println!("invite ended: {e}"),
            _ => {}
        },
        _ => {}
    }
}

async fn wait_ctrl_c(engine: &Engine, until_paired: bool) -> Result<()> {
    let mut rx = engine.subscribe();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => return Ok(()),
            ev = rx.recv() => match ev {
                Ok(EngineEvent::PairingRequest(req)) => {
                    println!(
                        "\n'{}' ({}) wants to join. Fingerprint: {}",
                        req.device_name, req.platform, req.fingerprint
                    );
                    let answer = tokio::task::spawn_blocking(|| {
                        print!("Approve? [y/N] ");
                        use std::io::Write;
                        let _ = std::io::stdout().flush();
                        let mut line = String::new();
                        let _ = std::io::stdin().read_line(&mut line);
                        line.trim().eq_ignore_ascii_case("y")
                    })
                    .await?;
                    engine.invite_respond(req.request_id, answer)?;
                }
                Ok(ev) => {
                    print_event(&ev);
                    if until_paired && matches!(ev, EngineEvent::PairingFinished { device: Some(_), .. }) {
                        // Give the first sync a moment, then exit.
                        tokio::time::sleep(Duration::from_secs(3)).await;
                        return Ok(());
                    }
                }
                Err(_) => {}
            }
        }
    }
}

async fn doctor(cli: &Cli) -> Result<()> {
    println!("Local addresses:");
    let ips = netutil::local_ips();
    if ips.is_empty() {
        println!("  none — this device isn't on a local network");
    }
    for ip in &ips {
        println!("  {ip}");
    }
    match std::net::TcpListener::bind(("0.0.0.0", cli.port)) {
        Ok(_) => println!("Port {}: free", cli.port),
        Err(e) => println!(
            "Port {}: unavailable ({e}) — another LocalPass instance may be running",
            cli.port
        ),
    }
    println!("Browsing mDNS for LocalPass devices (5s)...");
    match Discovery::start().and_then(|d| Ok((d.browse(SYNC_SERVICE)?, d))) {
        Err(e) => println!("  mDNS unavailable: {e}"),
        Ok((mut rx, _d)) => {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
            let mut n = 0;
            while let Ok(Some(ev)) = tokio::time::timeout_at(deadline, rx.recv()).await {
                if let DiscoveryEvent::Found(f) = ev {
                    n += 1;
                    println!("  found {} at {:?}", f.fullname, f.addrs);
                }
            }
            if n == 0 {
                println!("  nothing found. If other devices run LocalPass on this network:");
                println!(
                    "   - the network may block multicast or device-to-device traffic (common on work/guest Wi-Fi);"
                );
                println!(
                    "   - try `lp peer-add <ip>` with the address the other device shows, or use a phone hotspot."
                );
            }
        }
    }
    if cfg!(windows) {
        println!(
            "Windows: if other devices can't reach this one, allow LocalPass through Windows Defender Firewall"
        );
        println!("  (Settings > Privacy & security > Windows Security > Firewall > Allow an app).");
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let default_level = if matches!(cli.cmd, Cmd::Serve | Cmd::Invite { .. }) {
        "info"
    } else {
        "warn"
    };
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default_level)),
        )
        .with_target(false)
        .init();

    if let Cmd::Doctor = cli.cmd {
        return doctor(&cli).await;
    }
    if let Cmd::Gen { length, no_symbols } = cli.cmd {
        println!("{}", generated(length, !no_symbols)?);
        return Ok(());
    }

    let auto_approve = matches!(cli.cmd, Cmd::Invite { yes: true });
    let engine = open(&cli, auto_approve)?;
    match &cli.cmd {
        Cmd::Init { name } => {
            if engine.status() != VaultStatus::NoVault {
                bail!("this device already has a vault");
            }
            let pw = new_password(&cli)?;
            engine.create_vault(&pw, name)?;
            println!("Vault created. This device: {name}");
            println!("Next: run `lp invite` here, then `lp join <code>` on another device.");
        }
        Cmd::Join { code, addr, name } => {
            let pw = new_password(&cli)?;
            println!("Looking for the inviting device...");
            engine.join(code, addr, &pw, name).await?;
            println!(
                "Joined. {} items synced.",
                engine.list_items(&ItemQuery::default())?.len()
            );
        }
        Cmd::Status => {
            println!("Vault: {:?}", engine.status());
            if let Some(id) = engine.device_id() {
                println!(
                    "This device: {} ({})",
                    engine.device_name().unwrap_or_default(),
                    &id[..16]
                );
            }
            println!(
                "Addresses others can use: {:?}",
                netutil::local_addrs(cli.port)
            );
        }
        Cmd::List { search, trash } => {
            unlock(&cli, &engine)?;
            let items = engine.list_items(&ItemQuery {
                search: search.clone(),
                trash: *trash,
                ..Default::default()
            })?;
            for i in &items {
                let flags = format!(
                    "{}{}{}",
                    if i.favorite { "★" } else { " " },
                    if i.has_totp { "⏱" } else { " " },
                    if i.conflicted { "!" } else { " " }
                );
                println!("{}  {flags}  {:<30} {}", &i.id[..8], i.title, i.subtitle);
            }
            println!("{} items", items.len());
        }
        Cmd::Show { id } => {
            unlock(&cli, &engine)?;
            let id = resolve_id(&engine, id)?;
            let v = engine.get_item(&id)?;
            println!("{} ({})", v.title, v.kind);
            println!("  id:       {}", v.id);
            println!("  username: {}", v.username);
            println!("  password: {}", v.password);
            for u in &v.urls {
                println!("  url:      {u}");
            }
            if !v.totp.is_empty() {
                let code = engine.totp_code(&id)?;
                println!("  2FA code: {} ({}s left)", code.code, code.remaining);
            }
            if !v.notes.is_empty() {
                println!("  notes:    {}", v.notes);
            }
            println!("  updated by {} {}", v.updated_by, ago(Some(v.updated_ms)));
            for c in &v.conflicts {
                println!(
                    "  CONFLICT on {}: current '{}' vs:",
                    c.field, c.current.value
                );
                for a in &c.alternatives {
                    println!("     '{}' from {} ({})", a.value, a.device, a.op);
                }
            }
        }
        Cmd::Add {
            title,
            username,
            password: pw,
            generate,
            url,
            notes,
            totp,
        } => {
            unlock(&cli, &engine)?;
            let pw = match (pw, generate) {
                (Some(p), _) => p.clone(),
                (None, true) => generated(20, true)?,
                (None, false) => String::new(),
            };
            let id = engine.save_item(&ItemInput {
                title: title.clone(),
                username: username.clone(),
                password: pw,
                urls: url.clone(),
                notes: notes.clone(),
                totp: totp.clone(),
                ..Default::default()
            })?;
            println!("added {id}");
            sync_briefly(&engine).await;
        }
        Cmd::SetPassword {
            id,
            password: pw,
            generate,
        } => {
            unlock(&cli, &engine)?;
            let id = resolve_id(&engine, id)?;
            let v = engine.get_item(&id)?;
            let pw = match (pw, generate) {
                (Some(p), _) => p.clone(),
                (None, true) => generated(20, true)?,
                (None, false) => bail!("give --password or --generate"),
            };
            engine.save_item(&ItemInput {
                id: Some(v.id),
                kind: v.kind,
                title: v.title,
                username: v.username,
                password: pw,
                urls: v.urls,
                notes: v.notes,
                totp: v.totp,
                folder: v.folder,
                favorite: v.favorite,
                card: v.card,
                custom: v.custom,
            })?;
            println!("password updated");
            sync_briefly(&engine).await;
        }
        Cmd::Rm { id, purge } => {
            unlock(&cli, &engine)?;
            let id = resolve_id(&engine, id)?;
            if *purge {
                engine.purge_item(&id)?
            } else {
                engine.trash_item(&id)?
            }
            println!("{}", if *purge { "deleted" } else { "moved to trash" });
            sync_briefly(&engine).await;
        }
        Cmd::Totp { id } => {
            unlock(&cli, &engine)?;
            let id = resolve_id(&engine, id)?;
            let c = engine.totp_code(&id)?;
            println!("{} ({}s left)", c.code, c.remaining);
        }
        Cmd::Devices => {
            unlock(&cli, &engine)?;
            for d in engine.devices()? {
                println!(
                    "{} {:<20} {:<8} {}  last sync {}{}",
                    &d.id[..16],
                    d.name,
                    d.status,
                    d.fingerprint,
                    if d.this_device {
                        "-".to_string()
                    } else {
                        ago(d.last_sync_ms)
                    },
                    if d.this_device { "  (this device)" } else { "" }
                );
            }
        }
        Cmd::Revoke { device } => {
            unlock(&cli, &engine)?;
            let full = engine
                .devices()?
                .into_iter()
                .find(|d| d.id.starts_with(device.as_str()))
                .context("no such device")?;
            engine.revoke(&full.id)?;
            println!(
                "revoked {}; other devices learn this when they next sync",
                full.name
            );
            sync_briefly(&engine).await;
        }
        Cmd::Invite { .. } => {
            unlock(&cli, &engine)?;
            engine.start_sync().await?;
            let inv = engine.invite_start()?;
            println!(
                "On the new device run:\n  lp join {} --name <name>",
                inv.code_display
            );
            println!(
                "If it can't find this device, add: {}",
                inv.addrs
                    .iter()
                    .map(|a| format!("--addr {a}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
            println!(
                "Code expires in {} minutes. Ctrl+C to cancel.",
                inv.expires_in_secs / 60
            );
            wait_ctrl_c(&engine, true).await?;
            engine.invite_cancel();
        }
        Cmd::Serve => {
            engine.start_sync().await?;
            println!("Syncing in the background (vault stays locked). Ctrl+C to stop.");
            println!("Addresses: {:?}", engine.sync_status().listen_addrs);
            wait_ctrl_c(&engine, false).await?;
        }
        Cmd::Sync { addr } => {
            engine.start_sync().await?;
            match addr {
                Some(a) => {
                    let a = lp_engine::devices::parse_addr(a).context("expected ip or ip:port")?;
                    engine.sync_with(a, false).await?;
                    println!("synced with {a}");
                }
                None => {
                    if !cli.no_mdns {
                        tokio::time::sleep(Duration::from_secs(3)).await;
                    }
                    let results = engine.sync_now().await?;
                    if results.is_empty() {
                        println!("no devices found; try --addr <ip> or `lp doctor`");
                    }
                    for r in results {
                        match r.result {
                            Ok((rx, tx)) => println!("{}: received {rx}, sent {tx}", r.addr),
                            Err(e) => println!("{}: {e}", r.addr),
                        }
                    }
                }
            }
        }
        Cmd::Import {
            file,
            file_password_env,
            keyfile,
        } => {
            unlock(&cli, &engine)?;
            let bytes =
                std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
            let keyfile = keyfile.as_ref().map(std::fs::read).transpose()?;
            let needs_pw = bytes.starts_with(b"LPBACKUP") || lp_import_kind(&bytes) == Some("kdbx");
            let file_pw = match (file_password_env, needs_pw) {
                (Some(var), _) => Some(Zeroizing::new(
                    std::env::var(var).with_context(|| format!("${var} is not set"))?,
                )),
                (None, true) => Some(Zeroizing::new(rpassword::prompt_password(
                    "File password: ",
                )?)),
                (None, false) => None,
            };
            let r = engine.import_file(
                &bytes,
                file_pw.as_deref().map(|s| s.as_str()),
                keyfile.as_deref(),
                true,
            )?;
            println!(
                "{}: imported {}, skipped {} duplicates",
                r.format, r.imported, r.duplicates
            );
            for w in &r.warnings {
                println!("  warning: {w}");
            }
            println!(
                "Delete the source file if it isn't encrypted: it contains your passwords in plain text."
            );
            sync_briefly(&engine).await;
        }
        Cmd::Export {
            file,
            plain,
            file_password_env,
        } => {
            unlock(&cli, &engine)?;
            if *plain {
                std::fs::write(file, engine.export_json()?)?;
                println!(
                    "wrote UNENCRYPTED export to {} — keep it safe and delete it when done",
                    file.display()
                );
                let left_out = engine.passkeys_left_out_of_plain_export()?;
                if left_out > 0 {
                    println!(
                        "{left_out} passkey(s) are not in this file; only the encrypted backup carries them"
                    );
                }
            } else {
                let pw = match file_password_env {
                    Some(var) => Zeroizing::new(
                        std::env::var(var).with_context(|| format!("${var} is not set"))?,
                    ),
                    None => Zeroizing::new(rpassword::prompt_password("Backup password: ")?),
                };
                std::fs::write(file, engine.export_backup(&pw)?)?;
                println!("wrote encrypted backup to {}", file.display());
            }
        }
        Cmd::PeerAdd { addr, allow_public } => {
            engine.add_manual_peer(addr, *allow_public)?;
            println!("remembered {addr}");
        }
        Cmd::Leave { yes } => {
            if !yes {
                bail!("this deletes the vault from this device; re-run with --yes");
            }
            engine.leave_vault()?;
            println!("vault data removed from this device");
        }
        Cmd::Doctor | Cmd::Gen { .. } => unreachable!(),
    }
    engine.stop_sync();
    Ok(())
}

fn lp_import_kind(bytes: &[u8]) -> Option<&'static str> {
    bytes
        .starts_with(&[0x03, 0xd9, 0xa2, 0x9a])
        .then_some("kdbx")
}

/// Accepts a full id or a unique prefix.
fn resolve_id(engine: &Engine, prefix: &str) -> Result<String> {
    let all = engine.list_items(&ItemQuery::default())?;
    let trash = engine.list_items(&ItemQuery {
        trash: true,
        ..Default::default()
    })?;
    let matches: Vec<_> = all
        .iter()
        .chain(trash.iter())
        .filter(|i| i.id.starts_with(prefix))
        .collect();
    match matches.as_slice() {
        [one] => Ok(one.id.clone()),
        [] => bail!("no item with id {prefix}"),
        _ => bail!("{prefix} is ambiguous"),
    }
}

/// After a local edit, try to push it to reachable devices right away.
async fn sync_briefly(engine: &Engine) {
    if engine.start_sync().await.is_err() {
        return;
    }
    tokio::time::sleep(Duration::from_secs(2)).await;
    if let Ok(results) = engine.sync_now().await {
        let ok = results.iter().filter(|r| r.result.is_ok()).count();
        if ok > 0 {
            println!("pushed to {ok} device(s)");
        }
    }
}
