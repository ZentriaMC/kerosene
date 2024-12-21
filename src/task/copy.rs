use std::{collections::VecDeque, path::PathBuf};

use async_trait::async_trait;
use eyre::{eyre, Context};
use serde::Deserialize;
use structstruck::strike;
use tracing::trace;

use crate::task::KeroseneTaskInfo;

use super::{
    RunCommandOpts, StdinSource, StructuredTask, TaskContext, TaskContextInner, TaskResult,
};

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
        let (command, _use_pipe) = build_install_command(
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
        let stdin = match &self.src {
            CopyTaskSource::Content { content } => {
                Some(StdinSource::Bytes(content.as_bytes().into()))
            }
            CopyTaskSource::File { file, remote_src } if !*remote_src => {
                let file_path = resolve_local_file(&ctx, "files", file).await?;
                let reader =
                    std::fs::File::open(file_path).wrap_err("failed to open local file")?;

                Some(StdinSource::Reader(Box::new(reader)))
            }
            _ => None,
        };

        ctx.run_command_opts(RunCommandOpts {
            command,
            stdin,
            ..Default::default()
        })?;

        Ok(None)
    }
}

pub(crate) async fn resolve_local_file<'a>(
    ctx: &TaskContextInner,
    subdirectory: &'a str,
    name: &'a str,
) -> eyre::Result<PathBuf> {
    let name_path = PathBuf::from(name);
    let mut possible_paths = VecDeque::new();

    if name_path.is_relative() {
        possible_paths.push_front(ctx.play_basedir.join(subdirectory).join(&name_path));
        possible_paths.push_front(ctx.play_basedir.join(&name_path));

        // Prepend role directories if present
        for directory in ctx.resource_dirs.iter().rev() {
            possible_paths.push_front(directory.join(&name_path));
            possible_paths.push_front(directory.join(subdirectory).join(&name_path));
        }
    } else {
        possible_paths.push_front(name_path);
    }

    for path in possible_paths {
        trace!(name, subdirectory, possible_path = ?path, "resolving file");
        if std::fs::metadata(&path).is_ok() {
            trace!(name, subdirectory, possible_path = ?path, "file resolved");
            return Ok(path);
        }
    }

    Err(eyre!("could not find file specified as '{name}'"))
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
