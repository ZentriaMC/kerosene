use std::{
    collections::{HashMap, VecDeque},
    future::Future,
    pin::Pin,
    sync::Arc,
};

use async_trait::async_trait;
use serde::de::DeserializeOwned;
use serde_yaml::Value;
use tokio::sync::Mutex;
use tracing::{debug, trace};

pub mod copy;
pub mod curl;
pub mod import_tasks;
pub mod meta;
pub mod set_fact;
pub mod systemd;
pub mod template;

#[derive(Default)]
pub struct TaskContextInner {
    pub facts: HashMap<String, Value>,
    pub do_become_user: Option<String>,
    pub pending_handlers: VecDeque<String>,
}

impl TaskContextInner {
    pub fn consume_pending_handlers(&mut self) -> eyre::Result<()> {
        while let Some(handler_name) = self.pending_handlers.pop_front() {
            // TODO: run handler
            debug!(handler_name, "running handler");
        }

        Ok(())
    }

    pub fn run_remote_command(&self, command: Vec<&str>) -> eyre::Result<()> {
        trace!(?command, become = self.do_become_user, "running remotely");
        Ok(())
    }
}

pub type TaskContext = Arc<Mutex<TaskContextInner>>;
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
