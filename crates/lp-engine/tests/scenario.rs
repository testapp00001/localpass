//! The target story, end to end over real TCP on localhost (mDNS off):
//! a phone creates the vault and pairs a home laptop; at work it pairs a work
//! laptop while the home laptop is away; in the evening the work laptop syncs
//! directly with the home laptop, which has never seen it, while the phone is off.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use lp_core::crypto::KdfParams;
use lp_engine::{
    Engine, EngineConfig, EngineError, FileSecretStore, ItemInput, ItemQuery, VaultStatus,
};
use lp_sync::SyncError;
use lp_sync::service::SyncConfig;

fn engine(dir: &Path) -> Engine {
    let mut c = EngineConfig::new(dir, "test");
    c.sync = SyncConfig {
        port: 0,
        bind: std::net::Ipv4Addr::LOCALHOST,
        mdns: false,
        tick: Duration::from_millis(200),
        resync_every: Duration::from_secs(3600),
        addrbook_every: Duration::from_secs(3600),
    };
    c.kdf = KdfParams::insecure_for_tests();
    c.auto_approve_joins = true;
    Engine::open(c, Arc::new(FileSecretStore::new(dir.join("device.key")))).unwrap()
}

fn addr(e: &Engine) -> String {
    format!("127.0.0.1:{}", e.sync_status().port.expect("sync running"))
}

fn login(title: &str, user: &str, password: &str) -> ItemInput {
    ItemInput {
        title: title.into(),
        username: user.into(),
        password: password.into(),
        ..Default::default()
    }
}

fn titles(e: &Engine) -> Vec<String> {
    let mut t: Vec<String> = e
        .list_items(&ItemQuery::default())
        .unwrap()
        .into_iter()
        .map(|i| i.title)
        .collect();
    t.sort();
    t
}

fn find(e: &Engine, title: &str) -> String {
    e.list_items(&ItemQuery::default())
        .unwrap()
        .into_iter()
        .find(|i| i.title == title)
        .unwrap()
        .id
}

fn update_password(e: &Engine, id: &str, password: &str) {
    let v = e.get_item(id).unwrap();
    e.save_item(&ItemInput {
        id: Some(v.id),
        title: v.title,
        username: v.username,
        password: password.into(),
        urls: v.urls,
        ..Default::default()
    })
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn phone_home_work_story() {
    let (pd, hd, wd) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );

    // --- Day 1, home: the phone creates the vault and pairs the home laptop.
    let phone = engine(pd.path());
    phone.create_vault("phone master pw", "Phone").unwrap();
    phone.start_sync().await.unwrap();
    phone
        .save_item(&login("Gmail", "me@gmail.com", "gmail-1"))
        .unwrap();
    phone.save_item(&login("Bank", "me", "bank-1")).unwrap();

    let invite = phone.invite_start().unwrap();
    let home = engine(hd.path());
    home.join(
        &invite.code_display,
        &[addr(&phone)],
        "home master pw",
        "Home Laptop",
    )
    .await
    .unwrap();
    assert_eq!(home.status(), VaultStatus::Unlocked);
    assert_eq!(titles(&home), ["Bank", "Gmail"]);

    // --- Day 2, work: the home laptop is away; the phone pairs the work laptop.
    home.stop_sync();
    let invite = phone.invite_start().unwrap();
    let work = engine(wd.path());
    work.join(
        &invite.code,
        &[addr(&phone)],
        "work master pw",
        "Work Laptop",
    )
    .await
    .unwrap();
    assert_eq!(titles(&work), ["Bank", "Gmail"]);

    work.save_item(&login("Jira", "me@corp", "jira-1")).unwrap();
    update_password(&phone, &find(&phone, "Bank"), "bank-2");
    work.sync_with(addr(&phone).parse().unwrap(), false)
        .await
        .unwrap();
    assert_eq!(titles(&phone), ["Bank", "Gmail", "Jira"]);
    assert_eq!(
        work.get_item(&find(&work, "Bank")).unwrap().password,
        "bank-2"
    );

    // --- Evening, home: the phone is off. The home laptop has never seen the work laptop.
    phone.stop_sync();
    home.start_sync().await.unwrap();
    assert!(
        home.devices()
            .unwrap()
            .iter()
            .all(|d| d.name != "Work Laptop")
    );
    work.sync_with(addr(&home).parse().unwrap(), false)
        .await
        .unwrap();

    assert_eq!(titles(&home), ["Bank", "Gmail", "Jira"]);
    assert_eq!(
        home.get_item(&find(&home, "Bank")).unwrap().password,
        "bank-2"
    );
    let devices = home.devices().unwrap();
    let w = devices
        .iter()
        .find(|d| d.name == "Work Laptop")
        .expect("home learned about work laptop");
    assert_eq!(w.status, "active");
    assert_eq!(devices.len(), 3);

    // --- Concurrent edits on both laptops resolve the same way everywhere.
    let gmail = find(&home, "Gmail");
    update_password(&home, &gmail, "gmail-home");
    update_password(&work, &gmail, "gmail-work");
    home.sync_with(addr(&work).parse().unwrap(), false)
        .await
        .unwrap();
    let (hv, wv) = (
        home.get_item(&gmail).unwrap(),
        work.get_item(&gmail).unwrap(),
    );
    assert_eq!(hv.password, wv.password);
    assert_eq!(hv.conflicts.len(), 1, "concurrent change must be flagged");
    let loser = &hv.conflicts[0].alternatives[0];
    assert_ne!(loser.value, hv.password);
    assert!(["gmail-home", "gmail-work"].contains(&loser.value.as_str()));
    // Picking the other value resolves the conflict on both after a sync.
    home.resolve_conflict(&gmail, "password", &loser.op)
        .unwrap();
    home.sync_with(addr(&work).parse().unwrap(), false)
        .await
        .unwrap();
    assert_eq!(work.get_item(&gmail).unwrap().password, loser.value);
    assert!(work.get_item(&gmail).unwrap().conflicts.is_empty());

    // --- Sync keeps working while a device is locked.
    home.lock();
    work.save_item(&login("Wiki", "me@corp", "wiki-1")).unwrap();
    work.sync_with(addr(&home).parse().unwrap(), false)
        .await
        .unwrap();
    assert!(matches!(
        home.list_items(&ItemQuery::default()),
        Err(EngineError::Locked)
    ));
    home.unlock("home master pw").unwrap();
    assert!(titles(&home).contains(&"Wiki".to_string()));
    assert!(matches!(home.unlock("wrong"), Ok(())), "already unlocked");
    assert!(home.verify_password("home master pw").unwrap());
    assert!(!home.verify_password("not the password").unwrap());
    assert_eq!(
        home.status(),
        VaultStatus::Unlocked,
        "a check changes nothing"
    );
    home.lock();
    assert!(home.verify_password("home master pw").unwrap());
    assert!(!home.verify_password("not the password").unwrap());
    assert_eq!(
        home.status(),
        VaultStatus::Locked,
        "a check changes nothing"
    );
    assert!(matches!(
        home.unlock("not the password"),
        Err(EngineError::WrongPassword)
    ));
    home.unlock("home master pw").unwrap();

    // --- The phone comes back and catches up through the home laptop.
    phone.start_sync().await.unwrap();
    phone
        .sync_with(addr(&home).parse().unwrap(), false)
        .await
        .unwrap();
    assert_eq!(titles(&phone), titles(&home));
    assert_eq!(titles(&phone), titles(&work));
    assert_eq!(
        phone.get_item(&gmail).unwrap().password,
        home.get_item(&gmail).unwrap().password
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn revoked_device_is_refused_everywhere() {
    let (pd, hd, wd) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let phone = engine(pd.path());
    phone.create_vault("phone master pw", "Phone").unwrap();
    phone.start_sync().await.unwrap();
    let home = engine(hd.path());
    home.join(
        &phone.invite_start().unwrap().code,
        &[addr(&phone)],
        "home master pw",
        "Home",
    )
    .await
    .unwrap();
    let work = engine(wd.path());
    work.join(
        &phone.invite_start().unwrap().code,
        &[addr(&phone)],
        "work master pw",
        "Work",
    )
    .await
    .unwrap();
    home.sync_with(addr(&phone).parse().unwrap(), false)
        .await
        .unwrap();

    // The work laptop is lost; the home laptop revokes it.
    let work_id = work.device_id().unwrap();
    home.revoke(&work_id).unwrap();
    let status = |e: &Engine| {
        e.devices()
            .unwrap()
            .into_iter()
            .find(|d| d.id == work_id)
            .unwrap()
            .status
    };
    assert_eq!(status(&home), "revoked");

    // The revocation reaches the phone, which then refuses the work laptop too.
    home.sync_with(addr(&phone).parse().unwrap(), false)
        .await
        .unwrap();
    assert_eq!(status(&phone), "revoked");
    for target in [&phone, &home] {
        let err = work
            .sync_with(addr(target).parse().unwrap(), false)
            .await
            .unwrap_err();
        assert!(
            matches!(
                err,
                EngineError::Sync(SyncError::Remote(_) | SyncError::Revoked)
            ),
            "unexpected error: {err}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn wrong_code_is_rejected_and_invite_survives() {
    let (pd, jd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let phone = engine(pd.path());
    phone.create_vault("phone master pw", "Phone").unwrap();
    phone.start_sync().await.unwrap();
    let invite = phone.invite_start().unwrap();
    let wrong = if invite.code == "00000000" {
        "11111111"
    } else {
        "00000000"
    };

    let joiner = engine(jd.path());
    let err = joiner
        .join(wrong, &[addr(&phone)], "joiner master pw", "Laptop")
        .await
        .unwrap_err();
    assert!(
        matches!(err, EngineError::Sync(SyncError::WrongCode)),
        "unexpected error: {err}"
    );
    assert_eq!(joiner.status(), VaultStatus::NoVault);

    joiner
        .join(&invite.code, &[addr(&phone)], "joiner master pw", "Laptop")
        .await
        .unwrap();
    assert_eq!(joiner.status(), VaultStatus::Unlocked);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reopen_from_disk_and_change_password() {
    let dir = tempfile::tempdir().unwrap();
    let id = {
        let e = engine(dir.path());
        e.create_vault("first master pw", "Phone").unwrap();
        e.save_item(&login("Gmail", "me", "pw")).unwrap()
    };
    let e = engine(dir.path());
    assert_eq!(e.status(), VaultStatus::Locked);
    e.unlock("first master pw").unwrap();
    assert_eq!(e.get_item(&id).unwrap().password, "pw");
    e.change_master_password("first master pw", "second master pw")
        .unwrap();
    e.lock();
    assert!(matches!(
        e.unlock("first master pw"),
        Err(EngineError::WrongPassword)
    ));
    e.unlock("second master pw").unwrap();
    e.trash_item(&id).unwrap();
    assert!(titles(&e).is_empty());
    assert_eq!(
        e.list_items(&ItemQuery {
            trash: true,
            ..Default::default()
        })
        .unwrap()
        .len(),
        1
    );
    assert_eq!(e.empty_trash().unwrap(), 1);
    assert!(
        e.list_items(&ItemQuery {
            trash: true,
            ..Default::default()
        })
        .unwrap()
        .is_empty()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn import_export_and_backup_round_trip() {
    let (ad, bd) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let a = engine(ad.path());
    a.create_vault("first master pw", "Laptop").unwrap();
    let csv = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
               Work,1,login,Jira,,PIN: 42,0,https://jira.corp,me,jira-pw,JBSWY3DPEHPK3PXP\n\
               Work,0,login,Wiki,,,0,https://wiki.corp,me,wiki-pw,not-base32!\n\
               ,,note,Wifi,ssid pw,,0,,,,\n";
    let report = a.import_file(csv.as_bytes(), None, None, true).unwrap();
    assert_eq!(report.imported, 3);
    assert_eq!(
        report.warnings.len(),
        1,
        "bad TOTP is reported: {:?}",
        report.warnings
    );
    // Importing the same file again only finds duplicates.
    let again = a.import_file(csv.as_bytes(), None, None, true).unwrap();
    assert_eq!((again.imported, again.duplicates), (0, 3));
    assert_eq!(a.folders().unwrap().len(), 1);
    let jira = a.get_item(&find(&a, "Jira")).unwrap();
    assert!(jira.favorite);
    assert_eq!(jira.custom[0].label, "PIN");
    assert_eq!(a.totp_code(&jira.id).unwrap().code.len(), 6);

    // Encrypted backup restores into another vault.
    let blob = a.export_backup("backup password").unwrap();
    let b = engine(bd.path());
    b.create_vault("second master pw", "Phone").unwrap();
    assert!(
        b.import_file(&blob, Some("wrong password"), None, true)
            .is_err()
    );
    let restored = b
        .import_file(&blob, Some("backup password"), None, true)
        .unwrap();
    assert_eq!(restored.imported, 3);
    assert_eq!(titles(&b), titles(&a));
    assert_eq!(b.folders().unwrap()[0].name, "Work");

    // Plain export is Bitwarden JSON.
    let json = a.export_json().unwrap();
    assert!(json.contains("\"encrypted\": false") && json.contains("jira-pw"));
}

/// UI threads (Tauri commands, the Android bridge) call the engine from outside any async
/// runtime. A local write must still wake the sync service instead of panicking.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn saving_from_a_plain_thread_does_not_panic() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    e.create_vault("phone master pw", "Phone").unwrap();
    e.start_sync().await.unwrap();

    let worker = e.clone();
    std::thread::spawn(move || {
        let id = worker
            .save_item(&login("From a UI thread", "me", "pw-1"))
            .unwrap();
        worker.set_favorite(&id, true).unwrap();
        worker.rename_this_device("Renamed phone").unwrap();
        worker.add_manual_peer("127.0.0.1:9", false).unwrap();
        worker.invite_start().unwrap();
        worker.invite_cancel();
        worker.lock();
    })
    .join()
    .expect("a UI-thread call panicked outside a tokio runtime");
    e.unlock("phone master pw").unwrap();

    assert_eq!(titles(&e), vec!["From a UI thread".to_string()]);
}

/// RD-4: the Android background worker's round must not switch off the sync service that the app
/// asked for while the round was running (the user opened the app in the middle of it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_background_round_does_not_stop_sync_the_app_started_meanwhile() {
    let dir = tempfile::tempdir().unwrap();
    let e = engine(dir.path());
    e.create_vault("phone master pw", "Phone").unwrap();

    // A round on its own starts the service and stops it again.
    e.sync_once(Duration::from_millis(50)).await.unwrap();
    assert!(!e.sync_running(), "a lone round cleans up after itself");

    // The app starts sync while a round is waiting for peers: the service stays.
    let app = e.clone();
    let (round, started) = tokio::join!(e.sync_once(Duration::from_millis(400)), async move {
        tokio::time::sleep(Duration::from_millis(100)).await;
        app.start_sync().await
    });
    round.unwrap();
    started.unwrap();
    assert!(e.sync_running(), "the app's sync survived the round");

    // And stopping it stays possible.
    e.stop_sync();
    assert!(!e.sync_running());
}
