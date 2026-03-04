use async_trait::async_trait;
use eyre::{Context, eyre};
use serde::Deserialize;

use crate::{render, task::KeroseneTaskInfo};

use super::{
    RunCommandOpts, StdinSource, StructuredTask, TaskContext, TaskOutput, TaskResult,
    copy::{build_install_command, resolve_local_file},
};

#[derive(Debug, Deserialize)]
pub struct TemplateTask {
    #[serde(default, rename = "src")]
    pub file: Option<String>,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub remote_src: bool,
    pub dest: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub group: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
}

#[async_trait]
impl StructuredTask for TemplateTask {
    async fn run_structured(&self, context: TaskContext) -> TaskResult {
        let remote_src_file = match (&self.file, self.remote_src) {
            (Some(file), true) => Some(file),
            _ => None,
        };

        let (command, _use_pipe) = build_install_command(
            &self.dest,
            remote_src_file,
            self.owner.as_ref(),
            self.group.as_ref(),
            self.mode.as_ref(),
        );

        let mut environment = minijinja::Environment::new();
        environment.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);

        let ctx = context.lock().await;
        let (template_path, template_src) = if let Some(content) = &self.content {
            (
                "<inline>".to_string(),
                String::from_utf8(content.as_bytes().into())?,
            )
        } else if let Some(file) = &self.file {
            if !self.remote_src {
                let file_path = resolve_local_file(&ctx, "templates", file).await?;
                (
                    file_path.to_string_lossy().to_string(),
                    std::fs::read_to_string(file_path).wrap_err("failed to open local file")?,
                )
            } else {
                todo!("unsupported template task source: remote_src")
            }
        } else {
            return Err(eyre!("template task requires either 'src' or 'content'"));
        };

        let resolved_vars = render::resolve_vars(&ctx.merged_vars())?;
        let render_context = minijinja::Value::from_serialize(&resolved_vars);
        let rendered =
            environment.render_named_str(&template_path, &template_src, render_context)?;

        ctx.run_command_opts(RunCommandOpts {
            command,
            stdin: Some(StdinSource::Bytes(rendered.into_bytes())),
            ..Default::default()
        })?;

        Ok(TaskOutput::changed(None))
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.template", &["template"], &TemplateTask::run)
}
