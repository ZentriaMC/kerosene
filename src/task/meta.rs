use async_trait::async_trait;
use serde::Deserialize;
use structstruck::strike;
use tracing::{debug, warn};

use crate::task::KeroseneTaskInfo;

use super::{StructuredTask, TaskContext, TaskResult};

strike! {
    #[strikethrough[derive(Debug, Deserialize)]]
    pub struct MetaTask(enum MetaTaskAction {
        #![serde(rename_all = "snake_case")]
        FlushHandlers,
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
            MetaTaskAction::Unknown(action) => {
                warn!(?action, "unsupported meta action")
            }
        }

        Ok(None)
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.meta", &["meta"], &MetaTask::run)
}
