//! `fluxgram.render` end to end: the program's own file, its renders in its worker processes. A
//! worker that dies fails its render only — the next render starts a new one, and TDLib's client
//! goes on.

use std::fs;
use std::io::{BufRead, BufReader, Lines, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

fn sticker(name: &str) -> String {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/stickers");
    dir.join(name).to_string_lossy().into_owned()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("fluxgram-e2e-{}-{name}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn render(input: &str, output: &Path, extra: i64) -> String {
    json!({
        "@type": "fluxgram.render",
        "input": input,
        "output": output,
        "size": 64,
        "@extra": extra,
    })
    .to_string()
}

/// The program, running.
struct Helper {
    child: Child,
    input: Option<ChildStdin>,
    output: Lines<BufReader<ChildStdout>>,
}

impl Helper {
    fn start(env: &[(&str, &str)]) -> Helper {
        let mut child = Command::new(env!("CARGO_BIN_EXE_fluxgram-helper"))
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut helper = Helper {
            child,
            input,
            output,
        };
        assert_eq!(helper.next()["@type"], "fluxgram.ready");
        helper
    }

    fn send(&mut self, lines: &[String]) {
        let input = self.input.as_mut().unwrap();
        for line in lines {
            writeln!(input, "{line}").unwrap();
        }
        input.flush().unwrap();
    }

    fn next(&mut self) -> Value {
        let line = self.output.next().expect("a line").unwrap();
        serde_json::from_str(&line).unwrap()
    }

    /// The answers with these `@extra`s, in the order they come (other lines skipped).
    fn answers(&mut self, extras: &[i64]) -> Vec<Value> {
        let mut answers = Vec::new();
        while answers.len() < extras.len() {
            let line = self.next();
            if line["@extra"]
                .as_i64()
                .is_some_and(|extra| extras.contains(&extra))
            {
                answers.push(line);
            }
        }
        answers
    }

    /// Ends the input: the program finishes and exits with 0, in time.
    fn end(mut self) {
        drop(self.input.take());
        let start = Instant::now();
        for line in self.output.by_ref() {
            line.unwrap();
        }
        assert!(self.child.wait().unwrap().success());
        assert!(start.elapsed() < Duration::from_secs(6));
    }
}

#[test]
fn the_queue_answers_every_request() {
    let dir = scratch("queue");
    let webm = dir.join("w.fxa");
    let mut helper = Helper::start(&[]);
    helper.send(&[
        render(&sticker("sticker.tgs"), &dir.join("t.fxa"), 1),
        render(&sticker("sticker.webm"), &webm, 2),
        render(&sticker("sticker.webm"), &webm, 3),
        render(&sticker("missing.webm"), &dir.join("m.fxa"), 4),
        render(&sticker("sticker.tgs"), &dir.join("t2.fxa"), 5),
    ]);
    let answers = helper.answers(&[1, 2, 3, 4, 5]);
    for answer in &answers {
        let kind = if answer["@extra"] == 4 {
            "fluxgram.error"
        } else {
            "fluxgram.rendered"
        };
        assert_eq!(answer["@type"], kind, "{answer}");
    }
    assert_eq!(answers.len(), 5);
    assert!(dir.join("t.fxa").exists() && webm.exists() && dir.join("t2.fxa").exists());
    helper.end();
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_crashed_worker_fails_its_render_only() {
    let dir = scratch("crash");
    // A real sticker, so only the test's switch makes its worker abort.
    let crash = dir.join("crash.tgs");
    fs::copy(sticker("sticker.tgs"), &crash).unwrap();
    let crash = crash.to_string_lossy().into_owned();
    let mut helper = Helper::start(&[("FLUXGRAM_HELPER_TEST_ABORT", &crash)]);

    helper.send(&[render(&crash, &dir.join("crash.fxa"), 1)]);
    let failed = helper.answers(&[1]).remove(0);
    assert_eq!(failed["@type"], "fluxgram.error");
    assert_eq!(failed["message"], "Can't render: the renderer failed");
    assert!(!dir.join("crash.fxa").exists());

    // The next renders start a new worker; TDLib's side never noticed.
    helper.send(&[
        render(&sticker("sticker.webm"), &dir.join("w.fxa"), 2),
        render(&sticker("sticker.tgs"), &dir.join("t.fxa"), 3),
        json!({"@client": "t", "@type": "getOption", "name": "version", "@extra": 4}).to_string(),
    ]);
    for answer in helper.answers(&[2, 3, 4]) {
        match answer["@extra"].as_i64() {
            Some(4) => assert_eq!(answer["@type"], "optionValueString", "{answer}"),
            _ => assert_eq!(answer["@type"], "fluxgram.rendered", "{answer}"),
        }
    }
    helper.end();
    fs::remove_dir_all(&dir).ok();
}
