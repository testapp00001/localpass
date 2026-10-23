// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // A browser starts this program as its native-messaging helper for the extension.
    let args: Vec<String> = std::env::args().collect();
    if localpass_lib::native_host::wanted(&args) {
        std::process::exit(localpass_lib::native_host::run(&args));
    }
    localpass_lib::run()
}
