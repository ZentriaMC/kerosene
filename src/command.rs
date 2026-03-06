use std::{
    ffi::{OsStr, OsString},
    process::Command,
};

use eyre::{Context, eyre};
use tracing::{Level, debug};

/// SSH ControlPath template — tokens expanded by ssh itself:
/// %r = remote user, %h = host, %p = port
const SSH_CONTROL_PATH: &str = "/tmp/kerosene-ssh-%r@%h:%p";

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

    fn build_shell_command_string(
        elevate: Option<&[String]>,
        working_directory: Option<&OsString>,
        command: &OsStr,
        args: &[OsString],
    ) -> eyre::Result<String> {
        let mut parts: Vec<&str> = Vec::new();

        if let Some(elevate) = elevate {
            parts.extend(elevate.iter().map(String::as_str));
        }

        if let Some(chdir) = working_directory {
            parts.extend(["env", "--chdir"]);
            parts.push(
                chdir
                    .to_str()
                    .ok_or_else(|| eyre!("working directory is not valid UTF-8"))?,
            );
        }

        parts.push(
            command
                .to_str()
                .ok_or_else(|| eyre!("command is not valid UTF-8"))?,
        );
        for arg in args {
            parts.push(
                arg.to_str()
                    .ok_or_else(|| eyre!("argument is not valid UTF-8"))?,
            );
        }

        shlex::try_join(parts.iter().copied())
            .map_err(|e| eyre!("failed to shell-quote command: {e}"))
    }

    fn prepare_command(&self) -> eyre::Result<(OsString, Vec<OsString>)> {
        match self.target {
            CommandTarget::Local { dry, .. } | CommandTarget::Remote { dry, .. }
                if !self.read_only && *dry =>
            {
                Ok((OsString::from("true"), Default::default()))
            }

            CommandTarget::Local { elevate, .. } => {
                let shell_cmd = Self::build_shell_command_string(
                    elevate.as_deref(),
                    self.working_directory.as_ref(),
                    &self.command,
                    &self.args,
                )?;
                Ok((
                    OsString::from("sh"),
                    vec![OsString::from("-c"), OsString::from(shell_cmd)],
                ))
            }
            CommandTarget::Remote {
                hostname,
                user,
                port,
                ssh_key,
                ssh_extra_args,
                elevate,
                ..
            } => {
                let shell_cmd = Self::build_shell_command_string(
                    elevate.as_deref(),
                    self.working_directory.as_ref(),
                    &self.command,
                    &self.args,
                )?;

                let mut args: Vec<OsString> = vec![
                    "-oClearAllForwardings=yes".into(),
                    "-oControlMaster=auto".into(),
                    format!("-oControlPath={SSH_CONTROL_PATH}").into(),
                    "-oControlPersist=60s".into(),
                ];

                if let Some(port) = port {
                    args.push(format!("-oPort={port}").into());
                }

                if let Some(key) = ssh_key {
                    args.push(format!("-oIdentityFile={key}").into());
                }

                args.extend(ssh_extra_args.iter().map(OsString::from));

                args.push(OsString::from(if let Some(user) = user {
                    format!("{user}@{hostname}")
                } else {
                    hostname.to_owned()
                }));
                args.push(OsString::from(shell_cmd));

                Ok((OsString::from("ssh"), args))
            }
        }
    }

    pub fn to_command(&self) -> eyre::Result<Command> {
        let (command, args) = self.prepare_command()?;
        let working_directory = self.working_directory.as_ref();
        if tracing::enabled!(Level::DEBUG) {
            debug!(?command, ?args, ?working_directory, "running");
        }

        let mut cmd = std::process::Command::new(command);
        cmd.args(args);
        Ok(cmd)
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
        port: Option<u16>,
        ssh_key: Option<String>,
        ssh_extra_args: Vec<String>,
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
            Self::Remote {
                hostname,
                user,
                port,
                ssh_key,
                ssh_extra_args,
                dry,
                ..
            } if !*dry => {
                let local = CommandTarget::default();
                let mut cmd = PreparedCommand::new(&local, "ssh");
                cmd.arg(format!("-oControlPath={SSH_CONTROL_PATH}"));
                cmd.arg("-Oexit");
                if let Some(port) = port {
                    cmd.arg(format!("-oPort={port}"));
                }
                if let Some(key) = ssh_key {
                    cmd.arg(format!("-oIdentityFile={key}"));
                }
                cmd.args(ssh_extra_args);
                cmd.arg(if let Some(user) = user {
                    format!("{user}@{hostname}")
                } else {
                    hostname.to_owned()
                });
                let status = cmd
                    .to_command()?
                    .spawn()
                    .wrap_err("failed to spawn ssh")?
                    .wait()
                    .wrap_err("failed to wait for ssh to exit")?;

                return match status.code() {
                    Some(exit_code) if exit_code != 0 && exit_code != 255 => {
                        Err(eyre!("unexpected ssh exit code: {exit_code}"))
                    }
                    Some(_) | None => Ok(()),
                };
            }
            _ => {}
        }

        Ok(())
    }
}

