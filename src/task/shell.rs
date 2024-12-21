use async_trait::async_trait;
use serde::Deserialize;
use structstruck::strike;

use crate::task::KeroseneTaskInfo;

use super::{StructuredTask, TaskContext, TaskResult};

strike! {
    #[strikethrough[derive(Debug, Deserialize)]]
    pub struct ShellTask {
        pub cmd: String,
        pub chdir: Option<String>,
        #[serde(default = "default_executable")]
        pub executable: String,
    }
}

fn default_executable() -> String {
    "/bin/sh".to_string()
}

#[async_trait]
impl StructuredTask for ShellTask {
    async fn run_structured(&self, context: TaskContext) -> TaskResult {
        let ctx = context.lock().await;
        ctx.run_command(
            self.chdir.as_deref(),
            vec![self.executable.as_str(), "-c", self.cmd.as_str()],
        )?;

        Ok(None)
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.shell", &[
        "shell",
    ], &ShellTask::run)
}
