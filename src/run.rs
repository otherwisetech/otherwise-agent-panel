//! Running a chat turn: the CLI headless with its output parsed as it streams, stoppable.
use crate::args::Invocation;
use crate::provider::{Cli, child_path};
use crate::remote::Cancel;
use crate::stream::{AgentError, Parser, StreamEvent, classify};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// Run `inv` to the end, handing events to `on_event`. Returns the answer's text. `servers`
/// are the session's MCP server names (to tell tool names apart).
pub fn run(cli: Cli, inv: &Invocation, servers: Vec<String>, cancel: &Cancel, on_event: &mut dyn FnMut(StreamEvent)) -> Result<String, AgentError> {
    let mut cmd = Command::new(&inv.program);
    cmd.args(&inv.args).current_dir(&inv.cwd).env("PATH", child_path()).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    for (k, v) in &inv.env {
        cmd.env(k, v);
    }
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let res = (|| {
        std::fs::create_dir_all(&inv.cwd).map_err(|e| AgentError::Failed(e.to_string()))?;
        let child = cmd.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { AgentError::NotInstalled(cli) } else { AgentError::Failed(e.to_string()) })?;
        let child = Arc::new(Mutex::new(child));
        let (stdin, stdout, stderr) = {
            let mut c = child.lock().unwrap_or_else(|e| e.into_inner());
            (c.stdin.take(), c.stdout.take(), c.stderr.take())
        };
        // stdin on its own thread: a big inline image must not block reading the output
        let input = inv.stdin.clone();
        let writer = std::thread::spawn(move || {
            if let Some(mut s) = stdin {
                let _ = s.write_all(input.as_bytes());
            }
        });
        let err_reader = std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut e) = stderr {
                let _ = (&mut e).take(64 * 1024).read_to_string(&mut s);
                let _ = std::io::copy(&mut e, &mut std::io::sink());
            }
            s
        });
        // the watchdog kills the child when the user stops the run
        let done = Cancel::new();
        let watchdog = {
            let (child, cancel, done) = (child.clone(), cancel.clone(), done.clone());
            std::thread::spawn(move || {
                while !done.is_cancelled() {
                    if cancel.is_cancelled() {
                        let mut c = child.lock().unwrap_or_else(|e| e.into_inner());
                        #[cfg(unix)]
                        // SAFETY: signalling the child's own process group (it was started as its
                        // leader); a gone group yields ESRCH, ignored
                        unsafe {
                            libc::kill(-(c.id() as i32), libc::SIGKILL);
                        }
                        let _ = c.kill();
                        return;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(60));
                }
            })
        };
        let mut parser = Parser::new(cli, servers);
        let mut errors = Vec::new();
        if let Some(out) = stdout {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                for e in parser.line(&line) {
                    if let StreamEvent::Error(m) = &e {
                        errors.push(m.clone());
                    }
                    on_event(e);
                }
            }
        }
        let status = loop {
            if let Some(s) = child.lock().unwrap_or_else(|e| e.into_inner()).try_wait().map_err(|e| AgentError::Failed(e.to_string()))? {
                break s;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        done.cancel();
        let _ = watchdog.join();
        let _ = writer.join();
        let stderr = err_reader.join().unwrap_or_default();
        if cancel.is_cancelled() {
            return Err(AgentError::Cancelled);
        }
        if let Some(e) = errors.first() {
            return Err(classify(cli, &format!("{e}\n{stderr}")));
        }
        if !status.success() && parser.final_text.trim().is_empty() {
            return Err(classify(cli, if stderr.trim().is_empty() { "exited without an answer" } else { &stderr }));
        }
        Ok(parser.final_text.clone())
    })();
    for f in &inv.temp_files {
        let _ = std::fs::remove_file(f);
    }
    res
}
