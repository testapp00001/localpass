//! A small log file for the prompts the app shows on behalf of a browser or Windows (Windows
//! Hello, unlock, pairing approval): `prompts.log` in the data folder. The release app has no
//! console, so without this file there is nothing to read after a real test.
//!
//! Only events with the target `lp_prompt` are written. They name windows by program file name,
//! window class, process id and handle; they never contain window titles, paths, site names,
//! user names or anything from the vault. The file is cut to a fresh one when it passes 256 KB
//! (the old one is kept as `prompts.log.1`).

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::Mutex;

use tracing::{Level, Subscriber};
use tracing_subscriber::Layer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::fmt::{self, MakeWriter};
use tracing_subscriber::registry::LookupSpan;

const MAX_BYTES: u64 = 256 * 1024;

static FILE: Mutex<Option<File>> = Mutex::new(None);

/// Opens (or starts) the log in `dir`. Until this is called, events are dropped.
pub fn open(dir: &Path) {
    let path = dir.join("prompts.log");
    if fs::metadata(&path).is_ok_and(|m| m.len() > MAX_BYTES) {
        let _ = fs::rename(&path, dir.join("prompts.log.1"));
    }
    if let Ok(file) = OpenOptions::new().create(true).append(true).open(&path) {
        *FILE.lock().expect("prompt log") = Some(file);
    }
}

struct Sink;

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if let Some(file) = FILE.lock().expect("prompt log").as_mut() {
            let _ = file.write_all(buf);
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = FILE.lock().expect("prompt log").as_mut() {
            let _ = file.flush();
        }
        Ok(())
    }
}

struct SinkMaker;

impl<'a> MakeWriter<'a> for SinkMaker {
    type Writer = Sink;

    fn make_writer(&'a self) -> Sink {
        Sink
    }
}

/// The tracing layer that writes the prompt events to the file.
pub fn layer<S>() -> impl Layer<S>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
{
    fmt::layer()
        .with_ansi(false)
        .with_target(false)
        .with_writer(SinkMaker)
        .with_filter(Targets::new().with_target("lp_prompt", Level::INFO))
}
