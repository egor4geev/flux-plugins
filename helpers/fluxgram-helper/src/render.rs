//! `fluxgram.render` (README.md → Plugin → program): an animated sticker — a TGS (Lottie in gzip,
//! `tgs`) or a WEBM (VP9 with its alpha channel, `webm`), told apart by their first bytes —
//! rendered once into Flux's file of frames (`fxa`).
//!
//! The renderers run in worker processes (`worker`), not in the program: a crash or a hang on a
//! stranger's file fails that render only, and never reaches the TDLib clients. Renders run
//! [`WORKERS`] at a time, the rest waiting in turn, each on a thread of the program that keeps a
//! worker: the plugin's lines and TDLib's are never held up. A worker is started with the first
//! render and kept for the next ones; one that died or took too long is gone, and the next render
//! starts a new one. A thread with nothing to render for [`IDLE`] stops its worker and ends, so
//! an idle program has neither. A render asked for again while it waits or runs (the same output)
//! is done once and answered for each.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::path::Path;
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::relay::InputError;
use crate::worker::{self, Worker};
use crate::{tgs, webm};

/// The most renders at once: a worker each.
const WORKERS: usize = 2;
/// A worker with nothing to render is kept this long for the next one, then stopped.
const IDLE: Duration = Duration::from_secs(30);
/// A render's stack (in the worker): rlottie's parser and libvpx go deep on theirs.
pub const STACK: usize = 16 << 20;
/// The largest sticker file read (Telegram's are up to 64 KB for a TGS, 256 KB for a WEBM).
const MAX_INPUT: u64 = 32 << 20;
/// The most frames of a file: 20 s at 30 frames a second (stickers are up to 3 s).
pub const MAX_FRAMES: usize = 600;
/// The frames' rate when the request doesn't say.
pub const DEFAULT_FPS: u32 = 30;
/// The largest frames, on their longer side.
pub const MAX_SIZE: u32 = 2048;

/// A render the plugin asked for.
#[derive(Debug, PartialEq)]
pub struct Render {
    pub input: String,
    pub output: String,
    pub size: u32,
    pub fps: u32,
}

/// The frames asked for: `size` pixels on the longer side, at most `fps` a second.
pub struct Target {
    pub size: u32,
    pub fps: u32,
}

/// A render done: the frames' size and count.
#[derive(Debug, PartialEq)]
pub struct Rendered {
    pub width: u32,
    pub height: u32,
    pub frames: usize,
}

/// A render waiting or running, with the `@extra` of each request of it.
struct Job {
    render: Render,
    extras: Vec<Option<Value>>,
}

struct Queue {
    waiting: VecDeque<Job>,
    /// The outputs being rendered, with their requests' `@extra`s (one asked again joins).
    running: Vec<(String, Vec<Option<Value>>)>,
    /// The render threads, each with its worker once it has rendered.
    threads: usize,
    /// Of them, those waiting for a render (up to [`IDLE`]).
    idle: usize,
    /// The program ends: a thread with nothing to render stops at once.
    closing: bool,
}

static QUEUE: Mutex<Queue> = Mutex::new(Queue {
    waiting: VecDeque::new(),
    running: Vec::new(),
    threads: 0,
    idle: 0,
    closing: false,
});
/// Signaled when a render is queued, and at the end.
static WAKE: Condvar = Condvar::new();
/// Signaled when the last render thread ends.
static DONE: Condvar = Condvar::new();

fn queue() -> MutexGuard<'static, Queue> {
    QUEUE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Queues a render; its answer goes out through `write` (whole lines) when it's done. Never waits.
pub fn submit(render: Render, extra: Option<Value>, write: fn(&[u8])) {
    let mut queue = queue();
    if let Some((_, extras)) = queue
        .running
        .iter_mut()
        .find(|(output, _)| *output == render.output)
    {
        extras.push(extra);
        return;
    }
    if let Some(job) = queue
        .waiting
        .iter_mut()
        .find(|job| job.render.output == render.output)
    {
        job.extras.push(extra);
        return;
    }
    queue.waiting.push_back(Job {
        render,
        extras: vec![extra],
    });
    // A thread waiting with its worker takes it; another starts if more wait than that.
    if queue.idle > 0 {
        WAKE.notify_one();
    }
    if queue.waiting.len() > queue.idle && queue.threads < WORKERS {
        let spawned = thread::Builder::new()
            .name("render".into())
            .spawn(move || work(write));
        match spawned {
            Ok(_) => queue.threads += 1,
            Err(error) if queue.threads == 0 => {
                // No thread to run it: it is answered now.
                let job = queue.waiting.pop_back().unwrap();
                drop(queue);
                let message = format!("Can't render: no thread ({error})");
                for extra in job.extras {
                    write(
                        InputError::new(message.clone(), extra.as_ref())
                            .line()
                            .as_bytes(),
                    );
                }
            }
            // The running threads take it.
            Err(_) => {}
        }
    }
}

/// A render thread: takes the waiting renders in turn through its worker; with nothing to render
/// for [`IDLE`] (or at the end), stops the worker and ends.
fn work(write: fn(&[u8])) {
    let mut worker: Option<Worker> = None;
    loop {
        let render = {
            let mut queue = queue();
            let deadline = Instant::now() + IDLE;
            loop {
                if let Some(job) = queue.waiting.pop_front() {
                    queue.running.push((job.render.output.clone(), job.extras));
                    break Some(job.render);
                }
                let now = Instant::now();
                if queue.closing || now >= deadline {
                    // Counted out while the queue is held: a render queued from now on starts
                    // a thread of its own.
                    queue.threads -= 1;
                    if queue.threads == 0 {
                        DONE.notify_all();
                    }
                    break None;
                }
                queue.idle += 1;
                queue = WAKE
                    .wait_timeout(queue, deadline - now)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0;
                queue.idle -= 1;
            }
        };
        let Some(render) = render else {
            // Stopped and waited for: no process stays.
            drop(worker);
            return;
        };
        let result = run_in_worker(&mut worker, &render);
        let extras = {
            let mut queue = queue();
            let at = queue
                .running
                .iter()
                .position(|(output, _)| *output == render.output)
                .unwrap();
            queue.running.swap_remove(at).1
        };
        let mut lines = String::new();
        for extra in extras {
            lines.push_str(&answer(&render, &result, extra.as_ref()));
        }
        write(lines.as_bytes());
    }
}

/// A render through the thread's worker, started for it if there is none (the program's own
/// file, `--render-worker`); a worker that died or took too long is gone after it.
fn run_in_worker(worker: &mut Option<Worker>, render: &Render) -> Result<Rendered, String> {
    if worker.is_none() {
        let started = std::env::current_exe().and_then(|program| Worker::spawn(&program));
        match started {
            Ok(started) => *worker = Some(started),
            Err(error) => return Err(format!("Can't render: no renderer ({error})")),
        }
    }
    match worker.as_mut().unwrap().run(render, worker::JOB_TIMEOUT) {
        Ok(result) => result,
        Err(message) => {
            // Stopped and waited for, now: the next render starts a new one.
            *worker = None;
            Err(message)
        }
    }
}

/// The end of the program: the renders asked for are finished and answered first, up to
/// `deadline` (no half-written files left), and the workers stopped.
pub fn finish(deadline: Instant) {
    let mut queue = queue();
    queue.closing = true;
    WAKE.notify_all();
    while queue.threads > 0 {
        let now = Instant::now();
        if now >= deadline {
            eprintln!("fluxgram-helper: the renders didn't finish in time");
            return;
        }
        queue = DONE
            .wait_timeout(queue, deadline - now)
            .unwrap_or_else(PoisonError::into_inner)
            .0;
    }
}

/// The answer line: `fluxgram.rendered`, or `fluxgram.error` with the reason.
pub fn answer(render: &Render, result: &Result<Rendered, String>, extra: Option<&Value>) -> String {
    match result {
        Ok(rendered) => {
            let mut line = format!(
                "{{\"@type\":\"fluxgram.rendered\",\"output\":{},\"width\":{},\"height\":{},\"frames\":{}",
                Value::from(render.output.as_str()),
                rendered.width,
                rendered.height,
                rendered.frames
            );
            if let Some(extra) = extra {
                line.push_str(",\"@extra\":");
                line.push_str(&extra.to_string());
            }
            line.push_str("}\n");
            line
        }
        Err(message) => InputError::new(message.clone(), extra).line(),
    }
}

/// Renders a sticker file into its FXA file.
pub fn run(render: &Render) -> Result<Rendered, String> {
    let data = read(Path::new(&render.input))?;
    let output = Path::new(&render.output);
    if let Some(dir) = output.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        fs::create_dir_all(dir).map_err(write_error)?;
    }
    let target = Target {
        size: render.size,
        fps: render.fps,
    };
    if data.starts_with(&[0x1F, 0x8B]) {
        tgs::render(&data, &target, output)
    } else if data.starts_with(&[0x1A, 0x45, 0xDF, 0xA3]) {
        webm::render(&data, &target, output)
    } else {
        Err("Not a TGS or WEBM sticker".into())
    }
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut data = Vec::new();
    fs::File::open(path)
        .and_then(|file| file.take(MAX_INPUT + 1).read_to_end(&mut data))
        .map_err(|error| format!("Can't read the sticker: {error}"))?;
    if data.len() as u64 > MAX_INPUT {
        return Err("Not a sticker: over 32 MB".into());
    }
    Ok(data)
}

/// A failed write of the output.
pub fn write_error(error: io::Error) -> String {
    format!("Can't write the frames: {error}")
}

/// Every how many of the sticker's frames one is taken: as few as keep at most `fps` a second
/// (60 → 30: every other one), so the frames stay evenly paced.
pub fn step(rate: f64, fps: u32) -> usize {
    // A rate a hair over (29.97 → 30, times in milliseconds) still counts as within.
    ((rate / fps as f64 - 0.02).ceil() as usize).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fxa::tests::read;
    use std::path::PathBuf;

    /// The fixtures: a TGS of 2 s at 60 frames a second, 512 × 512 (a gradient fading out, a
    /// trim path, a mask); a WEBM of 1 s at 30, 256 × 256, VP9 with its alpha.
    fn sticker(name: &str) -> String {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/stickers");
        dir.join(name).to_string_lossy().into_owned()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("fluxgram-render-{}-{name}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn job(input: &str, output: &Path, size: u32, fps: u32) -> Render {
        Render {
            input: input.into(),
            output: output.to_string_lossy().into_owned(),
            size,
            fps,
        }
    }

    /// The frames' alpha is neither all opaque nor all transparent, and they move.
    fn check_frames(frames: &[Vec<u8>]) {
        let alphas: Vec<u8> = frames
            .iter()
            .flat_map(|f| f.chunks(4).map(|p| p[3]))
            .collect();
        assert!(alphas.contains(&0), "some pixels are transparent");
        assert!(alphas.contains(&255), "some are opaque");
        assert!(
            alphas.iter().any(|a| (1..255).contains(a)),
            "some are in between"
        );
        assert_ne!(frames[0], frames[frames.len() / 2], "the frames move");
    }

    #[test]
    fn renders_a_tgs() {
        let dir = scratch("tgs");
        let output = dir.join("deep/er/1-144.fxa");
        let rendered = run(&job(&sticker("sticker.tgs"), &output, 144, 30)).unwrap();
        // 120 frames at 60 a second: every other one.
        let expected = Rendered {
            width: 144,
            height: 144,
            frames: 60,
        };
        assert_eq!(rendered, expected);
        let (width, height, micros, frames) = read(&output).unwrap();
        assert_eq!(
            (width, height, micros, frames.len()),
            (144, 144, 33_333, 60)
        );
        check_frames(&frames);
        assert!(!dir.join("deep/er/1-144.fxa.tmp").exists());

        // 15 a second: every fourth.
        let rendered = run(&job(&sticker("sticker.tgs"), &output, 100, 15)).unwrap();
        assert_eq!(rendered.frames, 30);
        assert_eq!(read(&output).unwrap().2, 66_667);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn renders_a_webm() {
        let dir = scratch("webm");
        let output = dir.join("1-144.fxa");
        let rendered = run(&job(&sticker("sticker.webm"), &output, 144, 30)).unwrap();
        let expected = Rendered {
            width: 144,
            height: 144,
            frames: 30,
        };
        assert_eq!(rendered, expected);
        let (width, height, micros, frames) = read(&output).unwrap();
        assert_eq!((width, height, frames.len()), (144, 144, 30));
        assert!((33_000..=33_700).contains(&micros), "{micros}");
        check_frames(&frames);
        // Its own size, and larger.
        for size in [256, 320] {
            let rendered = run(&job(&sticker("sticker.webm"), &output, size, 30)).unwrap();
            assert_eq!((rendered.width, rendered.frames), (size, 30));
            check_frames(&read(&output).unwrap().3);
        }
        let rendered = run(&job(&sticker("sticker.webm"), &output, 144, 15)).unwrap();
        assert_eq!(rendered.frames, 15);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn errors() {
        let dir = scratch("errors");
        let output = dir.join("out.fxa");
        let error = |input: &str, output: &Path| run(&job(input, output, 144, 30)).unwrap_err();

        let text = dir.join("text.tgs");
        fs::write(&text, "not a sticker").unwrap();
        assert_eq!(
            error(&text.to_string_lossy(), &output),
            "Not a TGS or WEBM sticker"
        );
        let missing = dir.join("missing.tgs");
        assert!(error(&missing.to_string_lossy(), &output).starts_with("Can't read the sticker"));

        for (name, broken) in [
            ("sticker.tgs", "A broken TGS"),
            ("sticker.webm", "A broken WEBM"),
        ] {
            let data = fs::read(sticker(name)).unwrap();
            let cut = dir.join(format!("cut-{name}"));
            fs::write(&cut, &data[..data.len() / 2]).unwrap();
            let message = error(&cut.to_string_lossy(), &output);
            assert!(message.starts_with(broken), "{name}: {message}");
        }
        let garbage = dir.join("garbage.tgs");
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        std::io::Write::write_all(&mut gz, b"{\"v\":\"5\",\"layers\":[[[[").unwrap();
        fs::write(&garbage, gz.finish().unwrap()).unwrap();
        assert!(error(&garbage.to_string_lossy(), &output).starts_with("A broken TGS"));
        assert!(!output.exists());

        // A file where its folder should be.
        let blocked = text.join("out.fxa");
        for name in ["sticker.tgs", "sticker.webm"] {
            let message = error(&sticker(name), &blocked);
            assert!(message.starts_with("Can't write the frames"), "{message}");
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn steps() {
        assert_eq!(step(60.0, 30), 2);
        assert_eq!(step(30.0, 30), 1);
        assert_eq!(step(30.3, 30), 1);
        assert_eq!(step(29.97, 30), 1);
        assert_eq!(step(25.0, 30), 1);
        assert_eq!(step(50.0, 30), 2);
        assert_eq!(step(60.0, 24), 3);
        assert_eq!(step(0.5, 30), 1);
    }

    #[test]
    fn answers() {
        let render = Render {
            input: "/a/1.tgs".into(),
            output: "/b/1-320.fxa".into(),
            size: 320,
            fps: 30,
        };
        let done = Ok(Rendered {
            width: 320,
            height: 180,
            frames: 60,
        });
        assert_eq!(
            answer(&render, &done, Some(&Value::from(9))),
            "{\"@type\":\"fluxgram.rendered\",\"output\":\"/b/1-320.fxa\",\"width\":320,\"height\":180,\"frames\":60,\"@extra\":9}\n"
        );
        assert_eq!(
            answer(&render, &Err("Not a TGS or WEBM sticker".into()), None),
            "{\"@type\":\"fluxgram.error\",\"message\":\"Not a TGS or WEBM sticker\"}\n"
        );
    }
}
