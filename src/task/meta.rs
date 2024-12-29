use async_trait::async_trait;
use eyre::eyre;
use serde::Deserialize;
use structstruck::strike;
use tracing::{debug, warn};

use crate::task::KeroseneTaskInfo;

use super::{StructuredTask, TaskContext, TaskResult};

strike! {
    #[strikethrough[derive(Debug, Deserialize)]]
    pub struct MetaTask(enum MetaTaskAction {
        #![serde(rename_all = "snake_case")]
        ClearFacts,
        ClearHostErrors,
        EndBatch,
        EndHost,
        EndPlay,
        FlushHandlers,
        Noop,
        RefreshInventory,
        ResetConnection,
        #[serde(untagged)]
        Unknown(serde_yaml::Value),
    });
}

#[async_trait]
impl StructuredTask for MetaTask {
    async fn run_structured(&self, context: TaskContext) -> TaskResult {
        match &self.0 {
            MetaTaskAction::FlushHandlers => {
                debug!("flushing pending handlers");
                crate::run_handlers(context).await?;
            }
            MetaTaskAction::Noop => {}
            MetaTaskAction::ResetConnection => {
                debug!("triggering reset on command target");
                context.lock().await.command_target.reset().await?;
            }
            MetaTaskAction::Unknown(action) => {
                return Err(eyre!("unknown meta action: {:?}", action));
            }
            action => {
                warn!(?action, "unhandled meta action");
            }
        }

        Ok(None)
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.meta", &["meta"], &MetaTask::run)
}
