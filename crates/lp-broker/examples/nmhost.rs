//! The native-messaging helper as a stand-alone program, for tests. The real helper is
//! `localpass.exe` started by the browser (see `localpass_lib::native_host`).

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let code = lp_broker::nmhost::run(&args, &|| {
        Err(std::io::Error::other(
            "the test helper never starts the app",
        ))
    });
    std::process::exit(code);
}
