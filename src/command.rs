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

    fn prepare_command(&self) -> (OsString, Vec<OsString>) {
        match self.target {
            CommandTarget::Local { dry, .. } | CommandTarget::Remote { dry, .. }
                if !self.read_only && *dry =>
            {
                (OsString::from("true"), Default::default())
            }

            CommandTarget::Local { elevate: None, .. } => (self.command.clone(), self.args.clone()),
            CommandTarget::Local {
                elevate: Some(elevate),
                ..
            } => {
                let cmd = OsString::from(elevate.first().unwrap());
                let mut args = Vec::from_iter(elevate[1..].iter().map(OsString::from));
                args.push(self.command.clone());
                args.extend(self.args.clone());

                (cmd, args)
            }
            CommandTarget::Remote {
                hostname,
                user,
                elevate,
                ..
            } => {
                let ssh = OsString::from("ssh");
                let mut args = Vec::new();

                args.push(OsString::from(if let Some(user) = user {
                    format!("{user}@{hostname}")
                } else {
                    hostname.to_owned()
                }));

                if let Some(elevate) = elevate {
                    args.extend(elevate.iter().map(OsString::from));
                }

                if let Some(chdir) = &self.working_directory {
                    args.push(OsString::from("env"));
                    args.push(OsString::from("--chdir"));
                    args.push(chdir.to_owned());
                }

                // XXX: turns out when passing backslashes to ssh, they need
                //      to be escaped twice
                args.push(self.command.to_str().unwrap().replace("\\", "\\\\").into());
                for arg in &self.args {
                    args.push(arg.to_str().unwrap().replace("\\", "\\\\").into());
                }

                (ssh, args)
            }
        }
    }

    pub fn to_command(&self) -> Command {
        let (command, args) = self.prepare_command();
        let working_directory = self.working_directory.as_ref();
        if tracing::enabled!(Level::DEBUG) {
            debug!(?command, ?args, ?working_directory, "running");
        }

        let mut cmd = std::process::Command::new(command);
        if let (CommandTarget::Local { .. }, Some(working_directory)) =
            (self.target, working_directory)
        {
            cmd.current_dir(working_directory);
        }
        cmd.args(args);
        cmd
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
