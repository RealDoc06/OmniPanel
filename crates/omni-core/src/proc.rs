//! Running short-lived helper programs (`pactl`) from blocking code.

use std::io::{self, Read};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Like [`Command::output`], but a program still running after `limit` is
/// killed and reported as [`io::ErrorKind::TimedOut`]: a stuck sound server
/// must not stall its callers (or quitting the app) forever.
pub fn output_within(cmd: &mut Command, limit: Duration) -> io::Result<Output> {
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let deadline = Instant::now() + limit;
    let (stdout, stderr) = (read_in_background(child.stdout.take()), read_in_background(child.stderr.take()));
    let left = || deadline.saturating_duration_since(Instant::now());
    let output = stdout.recv_timeout(left()).and_then(|out| Ok((out, stderr.recv_timeout(left())?)));
    let status = match output {
        Ok(_) => loop {
            match child.try_wait()? {
                Some(status) => break Some(status),
                None if left().is_zero() => break None,
                None => std::thread::sleep(Duration::from_millis(5)),
            }
        },
        Err(_) => None,
    };
    match (status, output) {
        (Some(status), Ok((stdout, stderr))) => Ok(Output { status, stdout, stderr }),
        _ => {
            let _ = child.kill();
            let _ = child.wait();
            Err(io::Error::new(io::ErrorKind::TimedOut, "timed out"))
        }
    }
}

/// Drain a pipe on its own thread, so a full pipe can't block the program.
fn read_in_background(pipe: Option<impl Read + Send + 'static>) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = tx.send(bytes);
    });
    rx
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut cmd = Command::new("/bin/sh");
        cmd.args(["-c", script]);
        cmd
    }

    #[test]
    fn captures_large_output_and_status() {
        let out = output_within(&mut sh("head -c 200000 /dev/zero; echo oops >&2; exit 3"), Duration::from_secs(10));
        let out = out.unwrap();
        assert_eq!((out.stdout.len(), out.stderr.as_slice(), out.status.code()), (200_000, &b"oops\n"[..], Some(3)));
    }

    #[test]
    fn a_stuck_program_is_killed() {
        let started = Instant::now();
        let error = output_within(&mut sh("exec sleep 30"), Duration::from_millis(100)).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < Duration::from_secs(5));
    }
}
