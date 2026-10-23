//! Test driver for the prompts the app shows on behalf of a broker client (Windows only).
//! It talks to a running app that has its own `LOCALPASS_PROFILE`, so a real LocalPass is never
//! touched. Used to watch which window is in front while Windows Hello is up.
//!
//!   prompt_probe pair   <profile> <creds-file> [extension|provider]
//!       asks to pair, prints `CODE <digits>` and, once approved in the app, saves the credentials
//!   prompt_probe create <profile> <creds-file> <seconds>
//!       asks for a passkey (Windows Hello is shown), prints `SENT`, and gives up after
//!       <seconds> by dropping the connection, which must close the Hello dialog
//!
//! The credentials file holds a client id and secret in plain text: keep it in a temp folder.

#[cfg(windows)]
mod probe {
    use std::time::Duration;

    use lp_broker::ClientKind;
    use lp_broker::client::{Credentials, Options, connect_pipe, pair_pipe};
    use lp_core::passkey::b64u;
    use serde_json::json;
    use zeroize::Zeroizing;

    fn usage() -> ! {
        eprintln!("usage: prompt_probe pair <profile> <creds> [extension|provider]");
        eprintln!("       prompt_probe create <profile> <creds> <seconds>");
        std::process::exit(2)
    }

    pub async fn main() {
        let args: Vec<String> = std::env::args().skip(1).collect();
        let [mode, profile, creds, rest @ ..] = args.as_slice() else {
            usage()
        };
        match mode.as_str() {
            "pair" => {
                let kind = match rest.first().map(String::as_str) {
                    Some("extension") => ClientKind::Extension,
                    _ => ClientKind::Provider,
                };
                let options = Options::new("Prompt probe", kind);
                let (_client, paired) = pair_pipe(Some(profile), &options, |code| {
                    println!("CODE {code}");
                })
                .await
                .expect("pairing");
                let c = &paired.credentials;
                std::fs::write(
                    creds,
                    format!("{}\n{}\n", c.client_id, b64u::encode(&c.secret)),
                )
                .expect("write credentials");
                println!("PAIRED");
            }
            "create" => {
                let seconds: u64 = rest
                    .first()
                    .and_then(|s| s.parse().ok())
                    .unwrap_or_else(|| usage());
                let text = std::fs::read_to_string(creds).expect("read credentials");
                let mut lines = text.lines();
                let credentials = Credentials {
                    client_id: lines.next().expect("client id").to_string(),
                    secret: Zeroizing::new(
                        b64u::decode(lines.next().expect("secret")).expect("secret"),
                    ),
                };
                let options = Options::new("Prompt probe", ClientKind::Provider);
                let mut client = connect_pipe(Some(profile), &options, &credentials)
                    .await
                    .expect("login");
                println!("STATUS {:?}", client.status());
                let id = client
                    .send_request(&json!({
                        "op": "passkeyCreate", "origin": "https://example.com", "rpId": "example.com",
                        "rpName": "Example", "userHandle": b64u::encode(b"probe-handle"),
                        "userName": "probe", "algorithms": [-7], "uv": "required",
                    }))
                    .await
                    .expect("send");
                println!("SENT");
                match tokio::time::timeout(Duration::from_secs(seconds), client.response(id)).await
                {
                    Ok(Ok(_)) => println!("ANSWERED ok"),
                    Ok(Err(e)) => println!("ANSWERED error {e}"),
                    Err(_) => println!("CANCELLED by dropping the connection"),
                }
                drop(client);
                // Let the app notice the closed pipe before this process exits.
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            _ => usage(),
        }
    }
}

#[cfg(windows)]
#[tokio::main]
async fn main() {
    probe::main().await;
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}
