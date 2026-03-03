use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    fmt::Debug,
    future::Future,
    io::{Read, Write},
    ops::Deref,
    os::unix::process::ExitStatusExt,
    path::PathBuf,
    pin::Pin,
    process::Stdio,
    sync::Arc,
};

use async_trait::async_trait;
use eyre::Context;
use serde::de::DeserializeOwned;
use serde_yaml::Value;
use tokio::sync::Mutex;
use tracing::trace;

use crate::{
    command::{CommandExt, CommandTarget, PreparedCommand},
    serde::task::HandlerDescription,
};

pub mod copy;
pub mod curl;
pub mod import_tasks;
pub mod meta;
pub mod set_fact;
pub mod shell;
pub mod systemd;
pub mod template;

#[derive(Clone, Debug)]
pub enum TaskId {
    Task(&'static str),
    Unknown(&'static str),
    Alias {
        id: &'static str,
        alias: &'static str,
    },
}

impl TaskId {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Task(id) => id,
            Self::Unknown(id) => id,
            Self::Alias { id, .. } => id,
        }
    }
}

pub enum StdinSource {
    Reader(Box<dyn Read + Send>),
    Bytes(Vec<u8>),
}

#[derive(Default)]
pub struct RunCommandOpts<'a> {
    pub command: Vec<&'a str>,
    pub working_directory: Option<&'a str>,
    pub stdin: Option<StdinSource>,
    pub capture: bool,
}

impl Debug for RunCommandOpts<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RunCommandOpts")
            .field("command", &self.command)
            .field("working_directory", &self.working_directory)
            .field(
                "stdin",
                if self.stdin.is_some() {
                    &"present"
                } else {
                    &"absent"
                },
            )
            .field("capture", &self.capture)
            .finish_non_exhaustive()
    }
}

#[derive(Debug)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub rc: i32,
}

#[derive(Debug, Default)]
pub struct TaskContextInner {
    pub play_basedir: PathBuf,
    pub resource_dirs: VecDeque<PathBuf>,

    /// Four-layer variable system (lowest to highest precedence):
    /// 1. `role_defaults` — from `roles/<name>/defaults/main.yml`, scoped per role
    /// 2. `facts` — from `set_fact`, persists across the entire play
    /// 3. `role_play_vars` — from play's role definition `vars:`, scoped per role
    /// 4. `task_vars` — from `vars:` on individual tasks/handlers, scoped per task
    pub role_defaults: HashMap<String, Value>,
    pub facts: HashMap<String, Value>,
    pub role_play_vars: HashMap<String, Value>,
    pub task_vars: HashMap<String, Value>,

    pub command_target: CommandTarget,
    pub do_become_user: Option<String>,
    pub pending_handlers: VecDeque<String>,

    pub known_handlers: HashMap<String, HandlerDescription>,
}

impl TaskContextInner {
    /// Returns the effective variable set with Ansible-correct precedence:
    /// `task_vars` > `role_play_vars` > `facts` > `role_defaults`
    pub fn merged_vars(&self) -> HashMap<String, Value> {
        let mut merged = self.role_defaults.clone();
        merged.extend(self.facts.iter().map(|(k, v)| (k.clone(), v.clone())));
        merged.extend(
            self.role_play_vars
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        merged.extend(self.task_vars.iter().map(|(k, v)| (k.clone(), v.clone())));
        merged
    }

    pub fn run_command(
        &self,
        working_directory: Option<&str>,
        command: Vec<&str>,
    ) -> eyre::Result<CommandOutput> {
        self.run_command_opts(RunCommandOpts {
            command,
            working_directory,
            ..Default::default()
        })
    }

    pub fn run_command_opts(&self, opts: RunCommandOpts) -> eyre::Result<CommandOutput> {
        let RunCommandOpts {
            command,
            working_directory,
            stdin,
            capture,
        } = opts;

        trace!(?command, become = self.do_become_user, capture, "running command");

        // TODO: become_method
        let mut command_target = self.command_target.clone();
        if let Some(become_user) = &self.do_become_user {
            match &mut command_target {
                CommandTarget::Local { elevate, .. } => {
                    *elevate = Some(vec![
                        "sudo".to_string(),
                        format!("--user={}", become_user),
                        "--".to_string(),
                    ]);
                }
                CommandTarget::Remote { elevate, .. } => {
                    *elevate = Some(vec![
                        "sudo".to_string(),
                        format!("--user={}", become_user),
                        "--".to_string(),
                    ]);
                }
            }
        }

        let first = command.first().unwrap();
        let args = if command.len() > 1 {
            Vec::from(&command[1..])
        } else {
            Vec::new()
        };

        let mut child = PreparedCommand::new(&command_target, first)
            .chdir(working_directory.map(OsString::from))
            .args(args)
            .to_command()?
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(if capture {
                Stdio::piped()
            } else {
                Stdio::inherit()
            })
            .stderr(if capture {
                Stdio::piped()
            } else {
                Stdio::inherit()
            })
            .spawn()
            .wrap_err("failed to spawn child")?;

        if let Some(source) = stdin {
            let mut child_stdin = child.stdin.take().unwrap();
            match source {
                StdinSource::Bytes(bytes) => {
                    child_stdin
                        .write_all(&bytes)
                        .wrap_err("failed to write stdin")?;
                }
                StdinSource::Reader(mut reader) => {
                    std::io::copy(&mut reader, &mut child_stdin)
                        .wrap_err("failed to write stdin")?;
                }
            }
            // Drop stdin so the child sees EOF
            drop(child_stdin);
        }

        let output = child.wait_with_output().wrap_err("failed to wait for child")?;
        let rc = output
            .status
            .code()
            .unwrap_or_else(|| 128 + output.status.signal().unwrap_or(0));

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

        output.status.ensure_success()?;

        Ok(CommandOutput { stdout, stderr, rc })
    }
}

#[derive(Clone, Debug, Default)]
pub struct TaskContext {
    inner: Arc<Mutex<TaskContextInner>>,
}

impl Deref for TaskContext {
    type Target = Arc<Mutex<TaskContextInner>>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl TaskContext {
    pub fn new(play_basedir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Mutex::new(TaskContextInner {
                play_basedir,
                ..Default::default()
            })),
        }
    }
}

pub type TaskResult = eyre::Result<Option<Value>>;
pub type TaskFut = Pin<Box<dyn Future<Output = TaskResult> + Send + 'static>>;
pub type TaskRun = dyn Fn(TaskContext, Value) -> TaskFut + Send + Sync + 'static;

#[async_trait]
pub trait Task
where
    Self: Send + Sync,
{
    async fn run(context: TaskContext, value: Value) -> TaskResult;
}

#[async_trait]
pub trait StructuredTask
where
    Self: Send + Sync + DeserializeOwned,
{
    async fn run(context: TaskContext, value: Value) -> TaskResult {
        let parsed = Self::deserialize(value)?;

        parsed.run_structured(context).await
    }

    async fn run_structured(&self, context: TaskContext) -> TaskResult;
}

impl<S: StructuredTask> Task for S
where
    S: 'static,
{
    fn run<'async_trait>(
        context: TaskContext,
        value: Value,
    ) -> ::core::pin::Pin<
        Box<dyn ::core::future::Future<Output = TaskResult> + ::core::marker::Send + 'async_trait>,
    > {
        <S as StructuredTask>::run(context, value)
    }
}

pub struct KeroseneTaskInfo {
    pub fqdn: &'static str,
    pub aliases: Option<&'static [&'static str]>,
    pub run: &'static TaskRun,
}

inventory::collect!(KeroseneTaskInfo);

impl KeroseneTaskInfo {
    pub const fn new(fqdn: &'static str, run: &'static TaskRun) -> Self {
        Self {
            fqdn,
            aliases: None,
            run,
        }
    }

    pub const fn new_aliases(
        fqdn: &'static str,
        aliases: &'static [&'static str],
        run: &'static TaskRun,
    ) -> Self {
        Self {
            fqdn,
            aliases: Some(aliases),
            run,
        }
    }
}
