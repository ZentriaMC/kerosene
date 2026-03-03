use async_trait::async_trait;
use serde::Deserialize;
use serde_yaml::Value;
use structstruck::strike;

use crate::task::KeroseneTaskInfo;

use super::{RunCommandOpts, StructuredTask, TaskContext, TaskContextInner, TaskResult};

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

        let output = ctx.run_command_opts(RunCommandOpts {
            command: vec![executable, "-c", self.cmd.as_str()],
            working_directory: self.chdir.as_deref(),
            capture: true,
            ..Default::default()
        })?;

        let mut result = serde_yaml::Mapping::new();
        result.insert(
            Value::String("stdout".into()),
            Value::String(output.stdout.trim_end_matches('\n').into()),
        );
        result.insert(
            Value::String("stderr".into()),
            Value::String(output.stderr.trim_end_matches('\n').into()),
        );
        result.insert(
            Value::String("rc".into()),
            Value::Number(output.rc.into()),
        );

        Ok(Some(Value::Mapping(result)))
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.shell", &[
        "shell",
    ], &ShellTask::run)
}
