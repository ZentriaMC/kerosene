use async_trait::async_trait;
use serde::Deserialize;
use structstruck::strike;

use crate::task::KeroseneTaskInfo;

use super::{copy::build_install_command, StructuredTask, TaskContext, TaskResult};

strike! {
    #[strikethrough[derive(Debug, Deserialize)]]
    pub struct TemplateTask {
        #[serde(flatten)]
        pub src: enum TemplateTaskSource {
            #![serde(untagged)]
            File {
                #[serde(rename = "src")]
                file: String,

                #[serde(default)]
                remote_src: bool,
            }
            Content { content: String }
        },
        pub dest: String,
        #[serde(default)]
        pub owner: Option<String>,
        #[serde(default)]
        pub group: Option<String>,
        #[serde(default)]
        pub mode: Option<String>,
    }
}

#[async_trait]
impl StructuredTask for TemplateTask {
    async fn run_structured(&self, context: TaskContext) -> TaskResult {
        let (command, use_pipe) = build_install_command(
            &self.dest,
            match &self.src {
                TemplateTaskSource::File { file, remote_src } if *remote_src => Some(file),
                _ => None,
            },
            self.owner.as_ref(),
            self.group.as_ref(),
            self.mode.as_ref(),
        );

        let mut ctx = context.lock().await;
        ctx.run_remote_command(command)?;

        Ok(None)
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.template", &["template"], &TemplateTask::run)
}
