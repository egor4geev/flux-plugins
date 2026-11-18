//! fluxgram-helper: TDLib's JSON interface (`td_json_client`) behind JSON lines on stdin and
//! stdout, a TDLib client per account — the protocol is in README.md.
//!
//! Two threads: this one reads the plugin's lines and passes them to TDLib (`td_send` takes any
//! thread); the receiver waits in `td_receive` — no timers: an idle program sleeps — and writes
//! what's ready at once as one batch. At the end of input every client is closed first, so the
//! accounts' databases are left whole.

mod relay;
mod td;

use std::ffi::c_int;
use std::io::{self, BufRead, Write};
use std::sync::{Condvar, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use relay::{Batch, Clients, Input};
use serde_json::{Value, json};

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How long `td_receive` waits when nothing comes: a wakeup in ten minutes is the only cost of
/// an idle program.
const WAIT: f64 = 600.0;
/// The time the clients have to close at the end, all of them.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
/// TDLib's log file is rotated past this size.
const LOG_MAX: u64 = 10 << 20;

const USAGE: &str = "usage: fluxgram-helper [--log <file>] [--verbosity <0-5>] [--stats]";

/// The command line (README.md → Running).
struct Options {
    log: Option<String>,
    verbosity: u8,
    /// Each write's lines and bytes to stderr: for measuring the batches.
    stats: bool,
}

/// What both threads share: the clients, and a signal when one of them closes.
struct Shared {
    clients: Mutex<Clients>,
    closed: Condvar,
}

static SHARED: LazyLock<Shared> = LazyLock::new(|| Shared {
    clients: Mutex::new(Clients::default()),
    closed: Condvar::new(),
});

fn main() {
    let options = match options(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("fluxgram-helper: {message}\n{USAGE}");
            std::process::exit(2);
        }
    };

    // TDLib's log goes where it was asked to before any client starts.
    configure_log(&options);
    let tdlib = tdlib_version();
    eprintln!("fluxgram-helper {VERSION}: TDLib {tdlib}");
    write_out(
        format!(
            "{{\"@type\":\"fluxgram.ready\",\"version\":\"{VERSION}\",\"tdlib\":{}}}\n",
            Value::from(tdlib)
        )
        .as_bytes(),
    );

    let stats = options.stats;
    thread::Builder::new()
        .name("receiver".into())
        .spawn(move || receive(stats))
        .expect("a thread for TDLib's messages");

    read_input();
    finish();
}

fn options(mut args: impl Iterator<Item = String>) -> Result<Options, String> {
    let mut options = Options {
        log: None,
        verbosity: 1,
        stats: false,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--log" => options.log = Some(args.next().ok_or("--log needs a file")?),
            "--verbosity" => {
                options.verbosity = args
                    .next()
                    .and_then(|level| level.parse().ok())
                    .filter(|level| *level <= 5)
                    .ok_or("--verbosity needs a level from 0 to 5")?;
            }
            "--stats" => options.stats = true,
            "--help" | "-h" => {
                eprintln!("{USAGE}");
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(options)
}

/// TDLib's log level, and its file or none: TDLib writes to stderr otherwise.
fn configure_log(options: &Options) {
    execute_ok(json!({
        "@type": "setLogVerbosityLevel",
        "new_verbosity_level": options.verbosity,
    }));
    let empty = json!({"@type": "setLogStream", "log_stream": {"@type": "logStreamEmpty"}});
    match &options.log {
        Some(path) => {
            let file = json!({
                "@type": "setLogStream",
                "log_stream": {
                    "@type": "logStreamFile",
                    "path": path,
                    "max_file_size": LOG_MAX,
                    "redirect_stderr": false,
                },
            });
            if !execute_ok(file) {
                eprintln!("fluxgram-helper: TDLib can't write its log to {path}: no log");
                execute_ok(empty);
            }
        }
        None => {
            execute_ok(empty);
        }
    }
}

/// A synchronous request that answers `ok`; a failure is told on stderr.
fn execute_ok(request: Value) -> bool {
    let answer = td::execute(&request.to_string()).unwrap_or_default();
    let ok = serde_json::from_str::<Value>(&answer).is_ok_and(|answer| answer["@type"] == "ok");
    if !ok {
        eprintln!("fluxgram-helper: {} failed: {answer}", request["@type"]);
    }
    ok
}

/// The version of the TDLib linked in (`getOption` "version" answers synchronously).
fn tdlib_version() -> String {
    td::execute(r#"{"@type":"getOption","name":"version"}"#)
        .and_then(|answer| serde_json::from_str::<Value>(&answer).ok())
        .and_then(|answer| answer["value"].as_str().map(String::from))
        .unwrap_or_default()
}

/// The plugin's lines, until the input ends or `fluxgram.quit`.
fn read_input() {
    let mut input = io::stdin().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => {
                eprintln!("fluxgram-helper: input: {error}");
                return;
            }
        }
        if line.last() == Some(&b'\n') {
            line.pop();
        }
        match relay::parse_input(&line) {
            Ok(Input::Request { key, request }) => {
                let client = SHARED
                    .clients
                    .lock()
                    .unwrap()
                    .client(&key, td::create_client);
                td::send(client, &request);
            }
            Ok(Input::Execute { query, extra }) => {
                let result = td::execute(&query);
                write_out(relay::executed_line(result.as_deref(), extra.as_ref()).as_bytes());
            }
            Ok(Input::Filter(filter)) => SHARED.clients.lock().unwrap().set_filter(filter),
            Ok(Input::Quit) => return,
            Ok(Input::Nothing) => {}
            Err(error) => write_out(error.line().as_bytes()),
        }
    }
}

/// The receiver: waits for TDLib, then takes whatever else is ready at once and writes it all
/// with one write.
fn receive(stats: bool) {
    let mut batch = Batch::default();
    loop {
        let Some(first) = td::receive(WAIT) else {
            continue;
        };
        let messages = relay::gather(first, || td::receive(0.0));
        // stdout is held from here to the write: the end (`finish`) takes it before it exits, so
        // the last batch — fluxgram.closed — is written whole.
        let mut out = io::stdout().lock();
        let mut closed = false;
        {
            let mut clients = SHARED.clients.lock().unwrap();
            for message in &messages {
                closed |= clients.relay(message, &mut batch);
            }
        }
        if !batch.is_empty() {
            // The plugin may be gone: nothing to do about a failed write.
            let _ = out.write_all(batch.bytes()).and_then(|()| out.flush());
            if stats {
                eprintln!(
                    "fluxgram-helper: a write of {} lines, {} bytes",
                    batch.lines(),
                    batch.bytes().len()
                );
            }
            batch.clear();
        }
        drop(out);
        if closed {
            SHARED.closed.notify_all();
        }
    }
}

/// The end: every client is closed (TDLib's `close`: the databases are left whole), up to
/// [`CLOSE_TIMEOUT`] for all of them, then the program exits with 0.
fn finish() -> ! {
    let live = SHARED.clients.lock().unwrap().live_ids();
    for client in live {
        td::send(client, r#"{"@type":"close"}"#);
    }
    let deadline = Instant::now() + CLOSE_TIMEOUT;
    let mut clients = SHARED.clients.lock().unwrap();
    while !clients.is_empty() {
        let now = Instant::now();
        if now >= deadline {
            eprintln!("fluxgram-helper: TDLib's clients didn't close in 5 s");
            break;
        }
        clients = SHARED
            .closed
            .wait_timeout(clients, deadline - now)
            .unwrap()
            .0;
    }
    drop(clients);
    // Waits for the receiver's write in progress.
    let _ = io::stdout().lock().flush();
    // `_exit`: TDLib's threads are still there, and its static objects aren't destroyed under
    // them (`exit` would run their destructors); everything is written and closed by now.
    unsafe extern "C" {
        fn _exit(code: c_int) -> !;
    }
    unsafe { _exit(0) }
}

/// One write of whole lines, then a flush; a failed one is ignored (the plugin may be gone).
fn write_out(bytes: &[u8]) {
    let mut out = io::stdout().lock();
    let _ = out.write_all(bytes).and_then(|()| out.flush());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<Options, String> {
        options(args.iter().map(|arg| arg.to_string()))
    }

    #[test]
    fn command_line() {
        let default = parse(&[]).unwrap();
        assert_eq!(
            (default.log, default.verbosity, default.stats),
            (None, 1, false)
        );
        let all = parse(&["--log", "/tmp/td.log", "--verbosity", "4", "--stats"]).unwrap();
        assert_eq!(all.log.as_deref(), Some("/tmp/td.log"));
        assert_eq!((all.verbosity, all.stats), (4, true));
        assert!(parse(&["--verbosity", "6"]).is_err());
        assert!(parse(&["--verbosity"]).is_err());
        assert!(parse(&["--log"]).is_err());
        assert!(parse(&["--other"]).is_err());
    }
}
