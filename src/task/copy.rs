use async_trait::async_trait;
use serde::Deserialize;
use structstruck::strike;

use crate::task::KeroseneTaskInfo;

use super::{StructuredTask, TaskContext, TaskResult};

strike! {
    #[strikethrough[derive(Debug, Deserialize)]]
    pub struct CopyTask {
        #[serde(flatten)]
        pub src: enum CopyTaskSource {
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
impl StructuredTask for CopyTask {
    async fn run_structured(&self, context: TaskContext) -> TaskResult {
        let (command, use_pipe) = build_install_command(
            &self.dest,
            match &self.src {
                CopyTaskSource::File { file, remote_src } if *remote_src => Some(file),
                _ => None,
            },
            self.owner.as_ref(),
            self.group.as_ref(),
            self.mode.as_ref(),
        );

        let ctx = context.lock().await;
        ctx.run_remote_command(command)?;

        Ok(None)
    }
}

pub(crate) fn build_install_command<'a>(
    dest: &'a str,
    remote_src: Option<&'a String>,
    owner: Option<&'a String>,
    group: Option<&'a String>,
    mode: Option<&'a String>,
) -> (Vec<&'a str>, bool) {
    let mut command = vec!["install"];

    if let Some(owner) = owner {
        command.push("-o");
        command.push(owner.as_str());
    }

    if let Some(group) = group {
        command.push("-g");
        command.push(group.as_str());
    }

    if let Some(mode) = mode {
        command.push("-m");
        command.push(mode.as_str());
    }

    let use_pipe = if let Some(remote_src) = remote_src {
        command.push(remote_src);
        command.push(dest);
        false
    } else {
        command.push("/dev/stdin");
        command.push(dest);
        true
    };

    (command, use_pipe)
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.copy", &["copy"], &CopyTask::run)
}
