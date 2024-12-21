use async_trait::async_trait;
use eyre::Context;
use serde::Deserialize;
use structstruck::strike;

use crate::task::KeroseneTaskInfo;

use super::{
    copy::{build_install_command, resolve_local_file},
    RunCommandOpts, StdinSource, StructuredTask, TaskContext, TaskResult,
};

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
        let (command, _use_pipe) = build_install_command(
            &self.dest,
            match &self.src {
                TemplateTaskSource::File { file, remote_src } if *remote_src => Some(file),
                _ => None,
            },
            self.owner.as_ref(),
            self.group.as_ref(),
            self.mode.as_ref(),
        );

        let mut environment = minijinja::Environment::new();
        environment.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);

        let ctx = context.lock().await;
        let (template_path, template_src) = match &self.src {
            TemplateTaskSource::Content { content } => (
                "<inline>".to_string(),
                String::from_utf8(content.as_bytes().into())?,
            ),
            TemplateTaskSource::File { file, remote_src } if !*remote_src => {
                let file_path = resolve_local_file(&ctx, "templates", file).await?;

                (
                    file_path.to_string_lossy().to_string(),
                    std::fs::read_to_string(file_path).wrap_err("failed to open local file")?,
                )
            }
            _ => todo!("unsupported template task source"),
        };

        let render_context = minijinja::Value::from_serialize(&ctx.facts);
        let rendered =
            environment.render_named_str(&template_path, &template_src, render_context)?;

        eprintln!("{}", rendered);

        ctx.run_command_opts(RunCommandOpts {
            command,
            // TODO: streaming can be done too, but run_command_opts is too primitive currently to support minijinja's way.
            stdin: Some(StdinSource::Bytes(rendered.into_bytes())),
            ..Default::default()
        })?;

        Ok(None)
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.template", &["template"], &TemplateTask::run)
}
