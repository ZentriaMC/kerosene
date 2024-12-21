use std::{
    collections::{HashMap, VecDeque},
    ffi::OsString,
    future::Future,
    ops::Deref,
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

#[derive(Debug, Default)]
pub struct TaskContextInner {
    pub facts: HashMap<String, Value>,
    pub command_target: CommandTarget,
    pub do_become_user: Option<String>,
    pub pending_handlers: VecDeque<String>,

    pub known_handlers: HashMap<String, HandlerDescription>,
}

impl TaskContextInner {
    pub fn run_command(
        &self,
        working_directory: Option<&str>,
        command: Vec<&str>,
    ) -> eyre::Result<()> {
        trace!(?command, become = self.do_become_user, "running command");

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
            .to_command()
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .wrap_err("failed to spawn child")?;

        let _ = child.wait()?.ensure_success()?;

        Ok(())
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
    pub fn new() -> Self {
        Self {
            inner: Default::default(),
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
