//! Starting, waiting for, and stopping the web app under test.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anvil_core::config::qa::WebQaConfig;
use anyhow::{Context, Result, bail};

/// `true` when something answers HTTP at `url` within `timeout` (any status counts as an answer).
pub fn answers(url: &str, timeout: Duration) -> bool {
    let agent = ureq::AgentBuilder::new().timeout(timeout).redirects(0).build();
    match agent.get(url).call() {
        Ok(_) | Err(ureq::Error::Status(..)) => true,
        Err(ureq::Error::Transport(_)) => false,
    }
}

/// Starts the app from `qa.web.start_command` and waits until it answers. Returns `None` when the
/// app already answers, so a running dev server is never started twice.
pub fn ensure_app(root: &Path, web: &WebQaConfig) -> Result<Option<AppProcess>> {
    let Some(ready_url) = web.ready_url() else {
        bail!("`qa.web.url` is not set in .harness/config.json");
    };
    if answers(&ready_url, Duration::from_secs(2)) {
        return Ok(None);
    }
    let Some(command) = web.start_command.as_deref() else {
        bail!("{ready_url} does not answer and `qa.web.start_command` is not set");
    };
    let cwd = web.start_cwd.as_deref().map_or_else(|| root.to_path_buf(), |dir| root.join(dir));
    let log = super::run::runs_dir(root).join("app.log");
    let mut app = AppProcess::start(command, &cwd, &log)?;
    app.wait_ready(&ready_url, Duration::from_secs(web.ready_timeout_secs.max(1)))?;
    Ok(Some(app))
}

/// The app started from `qa.web.start_command`, stopped (with every child it spawned) on drop.
#[derive(Debug)]
pub struct AppProcess {
    child: Child,
    log_path: PathBuf,
}

impl AppProcess {
    /// Runs `command` through the system shell in `cwd`, logging its output to `log_path`.
    pub fn start(command: &str, cwd: &Path, log_path: &Path) -> Result<Self> {
        if let Some(parent) = log_path.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let log = File::create(log_path)
            .with_context(|| format!("failed to create {}", log_path.display()))?;
        let log_err = log.try_clone().context("failed to duplicate the log handle")?;
        let child = shell(command)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err))
            .spawn()
            .with_context(|| format!("failed to start `{command}` in {}", cwd.display()))?;
        Ok(Self { child, log_path: log_path.to_path_buf() })
    }

    /// Blocks until `ready_url` answers, the process exits, or `timeout` passes.
    pub fn wait_ready(&mut self, ready_url: &str, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            if answers(ready_url, Duration::from_secs(2)) {
                return Ok(());
            }
            if let Some(status) = self.child.try_wait().context("failed to poll the app process")? {
                bail!(
                    "the app exited ({status}) before answering at {ready_url}{}",
                    self.log_tail()
                );
            }
            if Instant::now() >= deadline {
                bail!(
                    "the app did not answer at {ready_url} within {}s{}",
                    timeout.as_secs(),
                    self.log_tail()
                );
            }
            thread::sleep(Duration::from_millis(300));
        }
    }

    fn log_tail(&self) -> String {
        let text = fs::read_to_string(&self.log_path).unwrap_or_default();
        let lines: Vec<&str> = text.lines().rev().take(8).collect();
        if lines.is_empty() {
            return String::new();
        }
        let tail: Vec<&str> = lines.into_iter().rev().collect();
        format!("; last output ({}):\n{}", self.log_path.display(), tail.join("\n"))
    }

    /// Stops the process tree. Safe to call more than once.
    pub fn stop(&mut self) {
        anvil_core::proc_util::kill_process_tree(&mut self.child);
    }
}

impl Drop for AppProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

fn shell(command: &str) -> Command {
    if cfg!(windows) {
        let mut cmd = Command::new("cmd");
        cmd.arg("/C");
        raw_arg(&mut cmd, command);
        cmd
    } else {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(command);
        cmd
    }
}

#[cfg(windows)]
fn raw_arg(cmd: &mut Command, text: &str) {
    use std::os::windows::process::CommandExt;
    cmd.raw_arg(text);
}

#[cfg(not(windows))]
fn raw_arg(cmd: &mut Command, text: &str) {
    cmd.arg(text);
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::TcpListener;

    use super::*;

    fn serve_once(status_line: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("addr").port();
        thread::spawn(move || {
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = write!(
                    stream,
                    "{status_line}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
        });
        format!("http://127.0.0.1:{port}/")
    }

    #[test]
    fn any_http_answer_counts_and_a_closed_port_does_not() {
        assert!(answers(&serve_once("HTTP/1.1 200 OK"), Duration::from_secs(2)));
        assert!(answers(&serve_once("HTTP/1.1 404 Not Found"), Duration::from_secs(2)));
        let closed = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            format!("http://127.0.0.1:{}/", listener.local_addr().expect("addr").port())
        };
        assert!(!answers(&closed, Duration::from_millis(500)));
    }

    #[test]
    fn an_app_that_exits_early_fails_the_wait_with_its_output() {
        let dir = tempfile::tempdir().expect("tmp");
        let mut app = AppProcess::start(
            "echo boot failure && exit 3",
            dir.path(),
            &dir.path().join("app.log"),
        )
        .expect("start");
        let err = app.wait_ready("http://127.0.0.1:1/", Duration::from_secs(10)).unwrap_err();
        let text = err.to_string();
        assert!(text.contains("exited"), "{text}");
        assert!(text.contains("boot failure"), "{text}");
    }
}
