use async_trait::async_trait;
use serde::Deserialize;
use structstruck::strike;

use crate::task::KeroseneTaskInfo;

use super::{StructuredTask, TaskContext, TaskContextInner, TaskResult};

strike! {
    #[strikethrough[derive(Debug, Deserialize)]]
    pub struct ShellTask {
        pub cmd: String,
        pub chdir: Option<String>,
        pub executable: Option<String>,
    }
}

fn default_executable(_ctx: &TaskContextInner) -> &'static str {
    // TODO: use ctx for determining executable per OS etc.
    "/bin/sh"
}

#[async_trait]
impl StructuredTask for ShellTask {
    async fn run_structured(&self, context: TaskContext) -> TaskResult {
        let ctx = context.lock().await;
        let executable = self
            .executable
            .as_deref()
            .unwrap_or(default_executable(&ctx));

        ctx.run_command(
            self.chdir.as_deref(),
            vec![executable, "-c", self.cmd.as_str()],
        )?;

        Ok(None)
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.shell", &[
        "shell",
    ], &ShellTask::run)
}
