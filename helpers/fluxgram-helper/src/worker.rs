//! The render worker: the renderers of `fluxgram.render` — rlottie and libvpx, C and C++ fed with
//! strangers' files — run in a process of their own, this program started again with
//! [`ARG`]. A crash there, or a sticker that never ends, costs that render only: the TDLib clients
//! and the sessions' keys stay in the program, out of the renderers' reach.
//!
//! The two talk in JSON lines, the protocol internal and tiny: a job on the worker's stdin
//! (`{"input":…,"output":…,"size":…,"fps":…}`), then its answer on its stdout
//! (`{"width":…,"height":…,"frames":…}` or `{"error":"…"}`), one job at a time. The worker ends
//! when its input does; its stderr is the program's.
//!
//! The program's side ([`Worker`]): a worker is started for the first render and kept for the
//! next; one that dies, or takes over [`JOB_TIMEOUT`], fails its job and is gone — the next job
//! starts a new one (`render` keeps them, and stops an idle one).

use std::ffi::{OsString, c_int};
use std::io::{self, BufRead, BufReader, Write};
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};

use crate::render::{self, Render, Rendered};

/// The command line of a worker: `fluxgram-helper --render-worker`.
pub const ARG: &str = "--render-worker";
/// A render that takes longer fails, and its worker is stopped (a sticker of 3 s renders in well
/// under a second).
pub const JOB_TIMEOUT: Duration = Duration::from_secs(15);
/// A worker ends by itself when a render outlasts this: past the program's [`JOB_TIMEOUT`], for
/// when the program is gone.
const SELF_TIMEOUT: Duration = Duration::from_secs(20);
/// The answer of a render whose worker died.
pub const FAILED: &str = "Can't render: the renderer failed";
/// The answer of a render that took over [`JOB_TIMEOUT`].
pub const TOO_LONG: &str = "Can't render: the renderer took too long";
/// Tests only: a worker aborts on a job whose input is this path, as a crash in the renderers
/// would end it.
const TEST_ABORT: &str = "FLUXGRAM_HELPER_TEST_ABORT";

/// The worker's side: its jobs from stdin, in turn, until the input ends. The renders run on a
/// thread with the stack they need (`render::STACK`; the main thread's is 1 MB on Windows); a
/// render that outlasts [`SELF_TIMEOUT`] ends the worker by itself — the program stops it sooner,
/// but may have ended meanwhile, and a worker is never left spinning.
pub fn serve() -> ! {
    let abort_on = std::env::var_os(TEST_ABORT);
    let (jobs, queued) = mpsc::channel::<Render>();
    let (results, done) = mpsc::channel();
    let renderer = thread::Builder::new()
        .name("render".into())
        .stack_size(render::STACK)
        .spawn(move || {
            for job in queued {
                // A bug's panic answers the job, and the worker goes on.
                let result = panic::catch_unwind(AssertUnwindSafe(|| render::run(&job)))
                    .unwrap_or_else(|_| Err(FAILED.into()));
                if results.send(result).is_err() {
                    return;
                }
            }
        });
    if renderer.is_ok() {
        serve_jobs(&jobs, &done, abort_on);
    } else {
        eprintln!("fluxgram-helper: no thread for the render worker");
    }
    exit()
}

fn serve_jobs(
    jobs: &mpsc::Sender<Render>,
    done: &Receiver<Result<Rendered, String>>,
    abort_on: Option<OsString>,
) {
    let mut input = io::stdin().lock();
    let mut out = io::stdout().lock();
    let mut line = Vec::new();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return,
        }
        let result = match job_from_line(&line) {
            Ok(job) => {
                if abort_on.as_deref() == Some(job.input.as_ref()) {
                    std::process::abort();
                }
                if jobs.send(job).is_err() {
                    return;
                }
                match done.recv_timeout(SELF_TIMEOUT) {
                    Ok(result) => result,
                    Err(_) => {
                        eprintln!("fluxgram-helper: a render took over 20 s: its worker ends");
                        return;
                    }
                }
            }
            Err(message) => Err(message),
        };
        let answer = answer_line(&result);
        if out
            .write_all(answer.as_bytes())
            .and_then(|()| out.flush())
            .is_err()
        {
            return;
        }
    }
}

/// `_exit`, as the program itself ends: nothing of TDLib's runs here, nothing to tear down — and a
/// render still running (one that never ends) doesn't hold the process.
fn exit() -> ! {
    unsafe extern "C" {
        fn _exit(code: c_int) -> !;
    }
    unsafe { _exit(0) }
}

/// A worker, from the program's side: the process, its input, and its answer lines (read on a
/// thread of their own, so a wait for one can end in time).
pub struct Worker {
    child: Child,
    input: ChildStdin,
    answers: Receiver<String>,
}

impl Worker {
    /// Starts `program` as a worker (the program's own file: `std::env::current_exe`).
    pub fn spawn(program: &Path) -> io::Result<Worker> {
        let mut command = Command::new(program);
        command.arg(ARG);
        Self::start(command)
    }

    fn start(mut command: Command) -> io::Result<Worker> {
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // No console window of its own (the program has none either).
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }
        let mut child = command.spawn()?;
        let (Some(input), Some(output)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(io::Error::other("no pipes to the worker"));
        };
        let (sender, answers) = mpsc::channel();
        let reader = thread::Builder::new()
            .name("render-worker".into())
            .spawn(move || {
                // Ends with the worker's output: the worker ended, or was stopped.
                for line in BufReader::new(output).lines() {
                    let Ok(line) = line else { return };
                    if sender.send(line).is_err() {
                        return;
                    }
                }
            });
        if let Err(error) = reader {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
        Ok(Worker {
            child,
            input,
            answers,
        })
    }

    /// Has the worker render a job: the render's own result, or `Err` with the job's answer when
    /// the worker died or took over `timeout` — the worker is unusable then (drop it).
    pub fn run(
        &mut self,
        job: &Render,
        timeout: Duration,
    ) -> Result<Result<Rendered, String>, String> {
        let line = job_line(job);
        if self
            .input
            .write_all(line.as_bytes())
            .and_then(|()| self.input.flush())
            .is_err()
        {
            return Err(FAILED.into());
        }
        match self.answers.recv_timeout(timeout) {
            Ok(answer) => Ok(answer_from_line(&answer)),
            Err(RecvTimeoutError::Timeout) => Err(TOO_LONG.into()),
            Err(RecvTimeoutError::Disconnected) => Err(FAILED.into()),
        }
    }
}

impl Drop for Worker {
    /// Stops the worker: it has no job then (an idle one), or failed one. Waited for, so no
    /// process is left behind.
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A job's line to the worker.
pub fn job_line(job: &Render) -> String {
    let mut line = json!({
        "input": job.input,
        "output": job.output,
        "size": job.size,
        "fps": job.fps,
    })
    .to_string();
    line.push('\n');
    line
}

/// A job line, as the worker reads it.
pub fn job_from_line(line: &[u8]) -> Result<Render, String> {
    let job: Value = serde_json::from_slice(line).map_err(|_| FAILED.to_string())?;
    let text = |key: &str| job[key].as_str().map(String::from);
    let number = |key: &str| job[key].as_u64().and_then(|n| u32::try_from(n).ok());
    match (text("input"), text("output"), number("size"), number("fps")) {
        (Some(input), Some(output), Some(size), Some(fps)) => Ok(Render {
            input,
            output,
            size,
            fps,
        }),
        _ => Err(FAILED.into()),
    }
}

/// The worker's answer line.
pub fn answer_line(result: &Result<Rendered, String>) -> String {
    let mut line = match result {
        Ok(rendered) => json!({
            "width": rendered.width,
            "height": rendered.height,
            "frames": rendered.frames,
        }),
        Err(message) => json!({"error": message}),
    }
    .to_string();
    line.push('\n');
    line
}

/// An answer line, as the program reads it: anything else is a failed render.
pub fn answer_from_line(line: &str) -> Result<Rendered, String> {
    let answer: Value = serde_json::from_str(line).map_err(|_| FAILED.to_string())?;
    if let Some(message) = answer["error"].as_str() {
        return Err(message.into());
    }
    let number = |key: &str| answer[key].as_u64().and_then(|n| u32::try_from(n).ok());
    match (number("width"), number("height"), number("frames")) {
        (Some(width), Some(height), Some(frames)) => Ok(Rendered {
            width,
            height,
            frames: frames as usize,
        }),
        _ => Err(FAILED.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn job() -> Render {
        Render {
            input: "/s/1 \"x\".tgs".into(),
            output: "/c/1-320.fxa".into(),
            size: 320,
            fps: 30,
        }
    }

    #[test]
    fn lines_both_ways() {
        let line = job_line(&job());
        assert!(line.ends_with('\n') && !line[..line.len() - 1].contains('\n'));
        assert_eq!(job_from_line(line.as_bytes()), Ok(job()));
        assert_eq!(job_from_line(b"{\"input\":\"a\"}\n"), Err(FAILED.into()));
        assert_eq!(job_from_line(b"garbage\n"), Err(FAILED.into()));

        let done = Ok(Rendered {
            width: 320,
            height: 180,
            frames: 60,
        });
        assert_eq!(
            answer_line(&done),
            "{\"frames\":60,\"height\":180,\"width\":320}\n"
        );
        assert_eq!(answer_from_line(answer_line(&done).trim_end()), done);
        let broken = Err("A broken TGS: not JSON".to_string());
        assert_eq!(answer_from_line(answer_line(&broken).trim_end()), broken);
        assert_eq!(answer_from_line("{}"), Err(FAILED.into()));
        assert_eq!(answer_from_line("Segmentation fault"), Err(FAILED.into()));
    }

    /// A worker that ends without an answer fails its job.
    #[cfg(unix)]
    #[test]
    fn a_worker_that_dies() {
        let mut worker = Worker::start(Command::new("/usr/bin/false")).unwrap();
        assert_eq!(
            worker.run(&job(), Duration::from_secs(5)),
            Err(FAILED.into())
        );
    }

    /// A worker that never answers fails its job in time, and is stopped when dropped.
    #[cfg(unix)]
    #[test]
    fn a_worker_that_hangs() {
        use std::time::Instant;

        let mut command = Command::new("/bin/sh");
        command.args(["-c", "exec sleep 30"]);
        let mut worker = Worker::start(command).unwrap();
        let start = Instant::now();
        assert_eq!(
            worker.run(&job(), Duration::from_millis(300)),
            Err(TOO_LONG.into())
        );
        assert!(start.elapsed() < Duration::from_secs(5));
        let pid = worker.child.id();
        drop(worker);
        // Gone (and waited for): no such process.
        let alive = Command::new("/bin/kill")
            .args(["-0", &pid.to_string()])
            .stderr(Stdio::null())
            .status()
            .unwrap()
            .success();
        assert!(!alive, "the worker {pid} is stopped");
    }
}
