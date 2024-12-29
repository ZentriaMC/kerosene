use std::{
    ffi::{OsStr, OsString},
    os::unix::process::ExitStatusExt,
    process::{Command, ExitStatus},
};

use eyre::eyre;
use tracing::{debug, Level};

pub struct PreparedCommand<'a> {
    pub target: &'a CommandTarget,
    pub command: OsString,
    pub args: Vec<OsString>,
    pub working_directory: Option<OsString>,
    // Flag that this command does not change system state
    pub read_only: bool,
}

impl<'a> PreparedCommand<'a> {
    pub fn new<S: AsRef<OsStr>>(target: &'a CommandTarget, cmd: S) -> Self {
        Self {
            target,
            command: cmd.as_ref().into(),
            args: Default::default(),
            working_directory: Default::default(),
            read_only: false,
        }
    }

    pub fn read_only(&mut self) -> &mut PreparedCommand<'a> {
        self.read_only = true;
        self
    }

    pub fn arg<S: AsRef<OsStr>>(&mut self, arg: S) -> &mut PreparedCommand<'a> {
        self.args.push(arg.as_ref().into());
        self
    }

    pub fn args<S, I>(&mut self, args: I) -> &mut PreparedCommand<'a>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        for arg in args {
            self.arg(arg);
        }
        self
    }

    pub fn chdir<S: AsRef<OsStr>>(
        &mut self,
        working_directory: Option<S>,
    ) -> &mut PreparedCommand<'a> {
        self.working_directory = working_directory.map(|v| v.as_ref().to_os_string());
        self
    }

    pub fn full_command(&self) -> Vec<&OsStr> {
        let mut cmd = vec![self.command.as_os_str()];
        cmd.extend(self.args.iter().map(OsString::as_os_str));
        cmd
    }

    pub fn to_command(&self) -> Command {
        if tracing::enabled!(Level::DEBUG) {
            debug!(command = ?self.full_command(), "running");
        }

        match self.target {
            CommandTarget::Local { dry, elevate } => {
                let mut cmd = if !self.read_only && *dry {
                    return std::process::Command::new("true");
                } else if let Some(elevate) = elevate {
                    let first = elevate.first().unwrap();
                    let args = &elevate[1..];

                    let mut cmd = std::process::Command::new(first);
                    cmd.args(args);
                    cmd.arg(&self.command);
                    cmd
                } else {
                    std::process::Command::new(&self.command)
                };

                cmd.args(&self.args);
                if let Some(chdir) = &self.working_directory {
                    cmd.current_dir(chdir);
                }
                cmd
            }
            CommandTarget::Remote {
                hostname,
                user,
                elevate,
                dry,
            } => {
                if !self.read_only && *dry {
                    return std::process::Command::new("true");
                }

                let target = if let Some(user) = user {
                    format!("{user}@{hostname}")
                } else {
                    hostname.to_owned()
                };

                let mut cmd = std::process::Command::new("ssh");
                cmd.arg(target);

                if let Some(chdir) = &self.working_directory {
                    cmd.args(["env", "--chdir"]);
                    cmd.arg(chdir);
                }

                if let Some(elevate) = elevate {
                    cmd.args(elevate);
                }

                // XXX: turns out when passing backslashes to ssh, they need
                //      to be escaped twice
                // c.args(&self.args);

                cmd.arg(self.command.to_str().unwrap().replace("\\", "\\\\"));
                for arg in &self.args {
                    cmd.arg(arg.to_str().unwrap().replace("\\", "\\\\"));
                }
                cmd
            }
        }
    }
}

impl From<PreparedCommand<'_>> for Command {
    fn from(value: PreparedCommand<'_>) -> Self {
        value.to_command()
    }
}

#[derive(Clone, Debug)]
pub enum CommandTarget {
    Local {
        elevate: Option<Vec<String>>,
        dry: bool,
    },
    Remote {
        hostname: String,
        user: Option<String>,
        elevate: Option<Vec<String>>,
        dry: bool,
    },
}

impl Default for CommandTarget {
    fn default() -> Self {
        Self::Local {
            elevate: None,
            dry: false,
        }
    }
}

impl CommandTarget {
    pub async fn reset(&self) -> eyre::Result<()> {
        match self {
            Self::Local { .. } => {}
            Self::Remote { hostname, dry, .. } if !*dry => {
                // TODO: ssh -O exit ${hostname}
            }
            _ => {}
        }

        Ok(())
    }
}

pub trait CommandExt: Sized {
    fn ensure_success(self) -> eyre::Result<Self>;
}

impl CommandExt for ExitStatus {
    fn ensure_success(self) -> eyre::Result<Self> {
        if !self.success() {
            let exit_code = self
                .code()
                // Add 128 like shells do
                .unwrap_or_else(|| 128 + self.signal().unwrap_or(0));

            Err(eyre!("unsuccessful run: exit status {}", exit_code))
        } else {
            Ok(self)
        }
    }
}
