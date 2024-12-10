use std::collections::HashMap;

use async_trait::async_trait;
use serde::Deserialize;
use serde_yaml::Value;
use structstruck::strike;
use tracing::debug;

use crate::task::KeroseneTaskInfo;

use super::{StructuredTask, TaskContext, TaskResult};

strike! {
    #[strikethrough[derive(Debug, Deserialize)]]
    pub struct SetFactTask {
        #[serde(flatten)]
        pub facts: HashMap<String, Value>,
    }
}

#[async_trait]
impl StructuredTask for SetFactTask {
    async fn run_structured(&self, context: TaskContext) -> TaskResult {
        let mut context = context.lock().await;

        for (key, value) in &self.facts {
            debug!(key, ?value, "setting fact");
            context.facts.insert(key.clone(), value.clone());
        }

        Ok(None)
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.set_fact", &["set_fact"], &SetFactTask::run)
}
