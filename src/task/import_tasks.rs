use async_trait::async_trait;
use serde::Deserialize;
use serde_yaml::Value;
use structstruck::strike;

use crate::task::KeroseneTaskInfo;

use super::{Task, TaskContext, TaskResult};

strike! {
    #[strikethrough[derive(Debug, Deserialize)]]
    pub struct ImportTasks {
        #[serde(flatten)]
        pub action: String,
    }
}

#[async_trait]
impl Task for ImportTasks {
    async fn run(context: TaskContext, value: Value) -> TaskResult {
        Ok(None)
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.import_tasks", &["import_tasks"], &ImportTasks::run)
}
