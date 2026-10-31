//! The broker's transport on Linux: a Unix socket in the user's runtime directory
//! (`$XDG_RUNTIME_DIR/localpass/broker-v1[-profile].sock`). It stands in for the Windows named pipe, with
//! the same promises:
//!
//! * only this user can reach it: the directory is created `0700`, and a directory that is not ours or
//!   that others can enter is refused (by the app before it listens, by clients before they connect);
//! * both ends check who is on the other side with `SO_PEERCRED`: the app drops a connection from
//!   another user, a client refuses a server that another user runs (phase 5.3, RB-5/RC-1);
//! * a second app on the same name is refused, and a socket a crashed app left behind is replaced.
//!
//! A process of this same user is the known same-user case: the secret handshake covers it once paired.

use std::ffi::OsStr;
use std::io;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::net::{UnixListener, UnixStream};

use super::{clean_profile, profile_from_env};

/// What a client holds once connected.
pub type Stream = UnixStream;

/// How long to wait for a socket that already exists to answer before calling the app "running".
const PROBE: Duration = Duration::from_millis(500);

fn our_uid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail.
    unsafe { libc::geteuid() }
}

/// The socket path for `profile` under `runtime_dir` (`$XDG_RUNTIME_DIR`). The directory must be set and
/// absolute: without it there is no private place for the socket, and none is invented.
pub fn socket_path_in(runtime_dir: Option<&OsStr>, profile: Option<&str>) -> io::Result<PathBuf> {
    let dir = runtime_dir
        .map(Path::new)
        .filter(|d| d.is_absolute())
        .ok_or_else(|| {
            io::Error::other(
                "XDG_RUNTIME_DIR is not set to an absolute path; LocalPass needs the per-user runtime \
                 folder for its browser connection",
            )
        })?;
    let file = match profile.and_then(clean_profile) {
        Some(p) => format!("broker-v1-{p}.sock"),
        None => "broker-v1.sock".to_string(),
    };
    Ok(dir.join("localpass").join(file))
}

pub fn socket_path(profile: Option<&str>) -> io::Result<PathBuf> {
    socket_path_in(std::env::var_os("XDG_RUNTIME_DIR").as_deref(), profile)
}

/// What `pipe_name_for` shows on Linux: the socket path, or why there is none.
pub fn name_for(profile: Option<&str>) -> String {
    match socket_path(profile) {
        Ok(path) => path.display().to_string(),
        Err(e) => format!("(no socket: {e})"),
    }
}

/// Whether a directory with these facts may hold the socket: a real directory (not a link), ours, and
/// closed to everyone else.
fn check_dir_facts(is_dir: bool, uid: u32, mode: u32, ours: u32) -> io::Result<()> {
    let refuse = |why: String| Err(io::Error::new(io::ErrorKind::PermissionDenied, why));
    if !is_dir {
        return refuse("the LocalPass socket folder is not a plain folder".into());
    }
    if uid != ours {
        return refuse("the LocalPass socket folder belongs to another account".into());
    }
    if mode & 0o077 != 0 {
        return refuse(format!(
            "the LocalPass socket folder can be entered by others (mode {:o})",
            mode & 0o7777
        ));
    }
    Ok(())
}

/// Checks `dir` without creating it; a missing folder is `NotFound` (the app has not run).
fn verify_private_dir(dir: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(dir)?;
    check_dir_facts(meta.is_dir(), meta.uid(), meta.mode(), our_uid())
}

fn ensure_private_dir(dir: &Path) -> io::Result<()> {
    match std::fs::DirBuilder::new().mode(0o700).create(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    verify_private_dir(dir)
}

fn peer_is_us(peer_uid: u32, ours: u32) -> io::Result<()> {
    if peer_uid == ours {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "the LocalPass socket is served by another account",
        ))
    }
}

fn check_peer(stream: &UnixStream) -> io::Result<()> {
    peer_is_us(stream.peer_cred()?.uid(), our_uid())
}

/// Removes the leftover of a crashed app, but only a socket that is ours.
fn remove_stale(path: &Path) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(path)?;
    if !meta.file_type().is_socket() || meta.uid() != our_uid() {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "something that is not a LocalPass socket is in the way",
        ));
    }
    std::fs::remove_file(path)
}

/// Creates the private folder if needed and listens on `path`. Fails with `AddrInUse` when an app
/// already answers there; replaces a socket nothing answers on.
pub async fn bind_private(path: &Path) -> io::Result<UnixListener> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no folder for the socket"))?;
    ensure_private_dir(dir)?;
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        Err(e) if e.kind() == io::ErrorKind::AddrInUse => {
            match tokio::time::timeout(PROBE, UnixStream::connect(path)).await {
                Ok(Ok(_)) | Err(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        "LocalPass is already running (its socket is in use)",
                    ));
                }
                Ok(Err(e))
                    if matches!(
                        e.kind(),
                        io::ErrorKind::ConnectionRefused | io::ErrorKind::NotFound
                    ) => {}
                Ok(Err(e)) => return Err(e),
            }
            remove_stale(path)?;
            UnixListener::bind(path)?
        }
        Err(e) => return Err(e),
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(listener)
}

/// Connects to the app's socket for `profile`.
pub async fn connect_profile(profile: Option<&str>) -> io::Result<UnixStream> {
    open(&socket_path(profile)?).await
}

/// Connects to the socket `LOCALPASS_PROFILE` selects (or the default).
pub async fn connect() -> io::Result<UnixStream> {
    connect_profile(profile_from_env().as_deref()).await
}

async fn open(path: &Path) -> io::Result<UnixStream> {
    if let Some(dir) = path.parent() {
        verify_private_dir(dir)?;
    }
    let stream = UnixStream::connect(path).await?;
    check_peer(&stream)?;
    Ok(stream)
}

#[cfg(feature = "server")]
mod listener {
    use std::io;
    use std::sync::Arc;
    use std::time::Duration;

    use tokio::sync::watch;
    use tracing::warn;

    use super::{bind_private, check_peer, socket_path};
    use crate::server::{Broker, Host};

    /// Deletes the socket file when the listener stops.
    struct RemoveOnDrop(std::path::PathBuf);

    impl Drop for RemoveOnDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    /// Accepts connections until `stop` changes. Returns an error if the socket can't be created
    /// (for example another app is already listening).
    pub async fn serve<H: Host>(
        broker: Arc<Broker<H>>,
        mut stop: watch::Receiver<bool>,
    ) -> io::Result<()> {
        let path = socket_path(super::profile_from_env().as_deref())?;
        let listener = bind_private(&path).await?;
        let _cleanup = RemoveOnDrop(path);
        loop {
            tokio::select! {
                accepted = listener.accept() => match accepted {
                    Ok((stream, _)) => {
                        if let Err(e) = check_peer(&stream) {
                            warn!(error = %e, "broker socket: connection refused");
                            continue;
                        }
                        let broker = broker.clone();
                        tokio::spawn(async move { broker.serve(stream).await });
                    }
                    Err(e) => {
                        warn!(error = %e, "broker socket accept failed");
                        tokio::time::sleep(Duration::from_millis(200)).await;
                    }
                },
                _ = stop.changed() => return Ok(()),
            }
        }
    }
}

#[cfg(feature = "server")]
pub use listener::serve;

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn the_socket_lives_in_a_localpass_folder_of_the_runtime_dir() {
        let run = OsStr::new("/run/user/1000");
        assert_eq!(
            socket_path_in(Some(run), None).unwrap(),
            Path::new("/run/user/1000/localpass/broker-v1.sock")
        );
        assert_eq!(
            socket_path_in(Some(run), Some("dev 2/../x")).unwrap(),
            Path::new("/run/user/1000/localpass/broker-v1-dev2x.sock"),
            "the profile is cleaned like the Windows pipe name"
        );
    }

    #[test]
    fn without_a_usable_runtime_dir_there_is_no_socket() {
        assert!(socket_path_in(None, None).is_err());
        assert!(socket_path_in(Some(OsStr::new("")), None).is_err());
        assert!(socket_path_in(Some(OsStr::new("relative/dir")), None).is_err());
    }

    #[test]
    fn a_folder_must_be_a_real_folder_of_ours_and_closed_to_others() {
        assert!(check_dir_facts(true, 1000, 0o700, 1000).is_ok());
        assert!(
            check_dir_facts(true, 1000, 0o100700, 1000).is_ok(),
            "type bits are ignored"
        );
        assert!(
            check_dir_facts(false, 1000, 0o700, 1000).is_err(),
            "a link or file"
        );
        assert!(
            check_dir_facts(true, 0, 0o700, 1000).is_err(),
            "another owner"
        );
        for mode in [0o755, 0o750, 0o705, 0o770, 0o777, 0o701] {
            assert!(
                check_dir_facts(true, 1000, mode, 1000).is_err(),
                "mode {mode:o}"
            );
        }
    }

    #[test]
    fn only_our_own_user_is_a_trusted_peer() {
        assert!(peer_is_us(1000, 1000).is_ok());
        let refused = peer_is_us(0, 1000).unwrap_err();
        assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn a_real_connection_comes_from_our_user() {
        let (a, b) = UnixStream::pair().unwrap();
        check_peer(&a).expect("the other end is this process");
        check_peer(&b).expect("and the other way round");
    }

    #[tokio::test]
    async fn binding_creates_a_private_folder_and_a_private_socket() {
        let root = tempdir();
        let path = root.path().join("localpass").join("a.sock");
        let _listener = bind_private(&path).await.unwrap();
        let dir_mode = std::fs::metadata(path.parent().unwrap()).unwrap().mode();
        assert_eq!(dir_mode & 0o777, 0o700);
        let sock = std::fs::metadata(&path).unwrap();
        assert!(sock.file_type().is_socket());
        assert_eq!(sock.mode() & 0o777, 0o600);
    }

    #[tokio::test]
    async fn a_folder_that_others_can_enter_is_refused() {
        let root = tempdir();
        let dir = root.path().join("localpass");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let err = bind_private(&dir.join("a.sock")).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        // Clients refuse to look for the app there as well.
        let err = open(&dir.join("a.sock")).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn a_folder_that_is_a_link_is_refused() {
        let root = tempdir();
        let real = root.path().join("real");
        std::fs::create_dir(&real).unwrap();
        let link = root.path().join("localpass");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let err = bind_private(&link.join("a.sock")).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
    }

    #[tokio::test]
    async fn a_second_listener_on_a_live_socket_is_refused_and_leaves_it_alone() {
        let root = tempdir();
        let path = root.path().join("localpass").join("a.sock");
        let first = bind_private(&path).await.unwrap();
        let err = bind_private(&path).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        // The first one still answers: it saw the second attempt's probe, then this client.
        let accept = tokio::spawn(async move {
            let _probe = first.accept().await?;
            let _client = first.accept().await?;
            io::Result::Ok(())
        });
        open(&path).await.expect("the first listener is intact");
        accept.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn a_socket_left_by_a_crashed_app_is_replaced() {
        let root = tempdir();
        let path = root.path().join("localpass").join("a.sock");
        let crashed = bind_private(&path).await.unwrap();
        drop(crashed); // the file stays, nothing listens
        assert!(path.exists());
        let _again = bind_private(&path)
            .await
            .expect("a stale socket is replaced");
        open(&path).await.expect("and the new one answers");
    }

    #[tokio::test]
    async fn something_else_at_the_path_is_never_deleted() {
        let root = tempdir();
        let dir = root.path().join("localpass");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = dir.join("a.sock");
        std::fs::write(&path, b"precious").unwrap();
        let err = bind_private(&path).await.unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AddrInUse);
        assert_eq!(std::fs::read(&path).unwrap(), b"precious");
    }

    #[tokio::test]
    async fn no_socket_means_the_app_is_not_running() {
        let root = tempdir();
        let missing_dir = open(&root.path().join("localpass").join("a.sock"))
            .await
            .unwrap_err();
        assert!(
            super::super::app_not_running(&missing_dir),
            "{missing_dir:?}"
        );
        let dir = root.path().join("localpass");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let missing_file = open(&dir.join("a.sock")).await.unwrap_err();
        assert!(
            super::super::app_not_running(&missing_file),
            "{missing_file:?}"
        );
        // A socket nothing listens on (a crashed app) counts too.
        drop(bind_private(&dir.join("a.sock")).await.unwrap());
        let stale = open(&dir.join("a.sock")).await.unwrap_err();
        assert!(super::super::app_not_running(&stale), "{stale:?}");
    }
}
