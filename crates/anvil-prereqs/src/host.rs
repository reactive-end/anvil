//! The machine the checks run on: processes, `PATH`, environment variables and the operating system.
//! Checks only talk to [`Host`], so tests swap in [`ScriptedHost`] and never spawn a process.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anvil_core::fs_util::path::bin_in_path;
use anvil_core::proc_util::run_command_with_timeout;

/// Operating system family the checks care about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Windows,
    MacOs,
    Other,
}

/// Result of running one program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// The program could not be found or started.
    NotFound,
    TimedOut,
    Ran {
        code: i32,
        stdout: String,
        stderr: String,
    },
}

/// Everything a check needs from the machine.
pub trait Host: Sync {
    /// Runs `program` (resolved on `PATH` unless absolute) with `args` under a wall-clock `timeout`.
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> RunOutcome;
    /// Runs a command line through the platform shell (`sh -c` or `cmd /C`).
    fn run_shell(&self, command: &str, timeout: Duration) -> RunOutcome;
    /// Resolves `name` on `PATH` (with `PATHEXT` on Windows).
    fn which(&self, name: &str) -> Option<PathBuf>;
    fn env_var(&self, name: &str) -> Option<String>;
    fn os(&self) -> HostOs;
}

/// The real machine.
pub struct SystemHost;

impl SystemHost {
    fn resolve(program: &str) -> Option<PathBuf> {
        let direct = Path::new(program);
        if direct.is_absolute() || program.contains(['/', '\\']) {
            let with_ext = direct.with_extension("exe");
            return [direct.to_path_buf(), with_ext].into_iter().find(|p| p.is_file());
        }
        bin_in_path(program)
    }

    fn outcome(cmd: &mut Command, timeout: Duration) -> RunOutcome {
        match run_command_with_timeout(cmd, timeout) {
            Ok(out) if out.timed_out => RunOutcome::TimedOut,
            Ok(out) => RunOutcome::Ran {
                code: out.status_code,
                stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            },
            Err(_) => RunOutcome::NotFound,
        }
    }
}

impl Host for SystemHost {
    fn run(&self, program: &str, args: &[&str], timeout: Duration) -> RunOutcome {
        match Self::resolve(program) {
            Some(path) => Self::outcome(Command::new(path).args(args), timeout),
            None => RunOutcome::NotFound,
        }
    }

    fn run_shell(&self, command: &str, timeout: Duration) -> RunOutcome {
        #[cfg(windows)]
        let mut cmd = {
            let mut c = Command::new("cmd");
            std::os::windows::process::CommandExt::raw_arg(&mut c, format!("/C {command}"));
            c
        };
        #[cfg(not(windows))]
        let mut cmd = {
            let mut c = Command::new("sh");
            c.args(["-c", command]);
            c
        };
        Self::outcome(&mut cmd, timeout)
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        bin_in_path(name)
    }

    fn env_var(&self, name: &str) -> Option<String> {
        std::env::var(name).ok().filter(|v| !v.is_empty())
    }

    fn os(&self) -> HostOs {
        if cfg!(windows) {
            HostOs::Windows
        } else if cfg!(target_os = "macos") {
            HostOs::MacOs
        } else {
            HostOs::Other
        }
    }
}

struct Reply {
    program: String,
    args: String,
    code: i32,
    stdout: String,
    stderr: String,
}

/// A fake machine with canned command output, for tests.
pub struct ScriptedHost {
    replies: Vec<Reply>,
    env: HashMap<String, String>,
    on_path: HashSet<String>,
    os: HostOs,
}

const SHELL: &str = "<shell>";

impl Default for ScriptedHost {
    fn default() -> Self {
        Self {
            replies: Vec::new(),
            env: HashMap::new(),
            on_path: HashSet::new(),
            os: HostOs::Other,
        }
    }
}

impl ScriptedHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// `program args...` exits 0 printing `stdout`.
    pub fn tool(self, program: &str, args: &str, stdout: &str) -> Self {
        self.reply(program, args, 0, stdout, "")
    }

    /// `program args...` exits `code` printing `stdout` and `stderr`.
    pub fn reply(
        mut self,
        program: &str,
        args: &str,
        code: i32,
        stdout: &str,
        stderr: &str,
    ) -> Self {
        self.replies.push(Reply {
            program: program.to_owned(),
            args: args.to_owned(),
            code,
            stdout: stdout.to_owned(),
            stderr: stderr.to_owned(),
        });
        self
    }

    /// The shell command `command` exits with `code`.
    pub fn shell(self, command: &str, code: i32) -> Self {
        self.reply(SHELL, command, code, "", "")
    }

    /// `name` resolves on `PATH` without a canned reply.
    pub fn on_path(mut self, name: &str) -> Self {
        self.on_path.insert(name.to_owned());
        self
    }

    pub fn env(mut self, name: &str, value: &str) -> Self {
        self.env.insert(name.to_owned(), value.to_owned());
        self
    }

    pub fn with_os(mut self, os: HostOs) -> Self {
        self.os = os;
        self
    }

    fn knows(&self, program: &str) -> bool {
        self.on_path.contains(program) || self.replies.iter().any(|r| r.program == program)
    }
}

impl Host for ScriptedHost {
    fn run(&self, program: &str, args: &[&str], _timeout: Duration) -> RunOutcome {
        if !self.knows(program) {
            return RunOutcome::NotFound;
        }
        let wanted = args.join(" ");
        match self.replies.iter().find(|r| r.program == program && r.args == wanted) {
            Some(r) => {
                RunOutcome::Ran { code: r.code, stdout: r.stdout.clone(), stderr: r.stderr.clone() }
            }
            None => RunOutcome::Ran { code: 1, stdout: String::new(), stderr: String::new() },
        }
    }

    fn run_shell(&self, command: &str, _timeout: Duration) -> RunOutcome {
        match self.replies.iter().find(|r| r.program == SHELL && r.args == command) {
            Some(r) => {
                RunOutcome::Ran { code: r.code, stdout: r.stdout.clone(), stderr: r.stderr.clone() }
            }
            None => RunOutcome::Ran { code: 127, stdout: String::new(), stderr: String::new() },
        }
    }

    fn which(&self, name: &str) -> Option<PathBuf> {
        self.knows(name).then(|| PathBuf::from("/fake/bin").join(name))
    }

    fn env_var(&self, name: &str) -> Option<String> {
        self.env.get(name).cloned()
    }

    fn os(&self) -> HostOs {
        self.os
    }
}
