use async_trait::async_trait;
use eyre::OptionExt;
use serde::Deserialize;
use structstruck::strike;

use crate::task::KeroseneTaskInfo;

use super::{StructuredTask, TaskContext, TaskResult};

strike! {
    #[strikethrough[derive(Debug, Deserialize)]]
    pub struct SystemdTask {
        #[serde(alias = "daemon-reload")]
        pub daemon_reload: Option<bool>,
        pub enabled: Option<bool>,
        pub force: Option<bool>,
        pub masked: Option<bool>,
        pub name: Option<String>,
        /// NOTE: applies only to state changes!
        pub no_block: Option<bool>,
        pub scope: Option<pub enum {
            #![serde(rename_all = "snake_case")]
            System,
            User,
            Global,
        }>,
        pub state: Option<pub enum {
            #![serde(rename_all = "snake_case")]
            Reloaded,
            Restarted,
            Started,
            Stopped,
        }>,
    }
}

#[async_trait]
impl StructuredTask for SystemdTask {
    async fn run_structured(&self, context: TaskContext) -> TaskResult {
        let scope_flag = match &self.scope {
            Some(Scope::Global) => "--global",
            Some(Scope::User) => "--user",
            _ => "--system",
        };

        if self.daemon_reload.unwrap_or_default() {
            let ctx = context.lock().await;
            ctx.run_command(None, vec!["systemctl", scope_flag, "daemon-reload"])?;
        }

        if let Some(enabled) = self.enabled {
            let name = self
                .name
                .as_ref()
                .ok_or_eyre("systemd service name is required")?;

            let mut command = vec!["systemctl", scope_flag];
            if enabled {
                command.push("enable");
            } else {
                command.push("disable");
            }

            if self.force.unwrap_or_default() {
                command.push("--force");
            }

            command.push(name.as_str());

            let ctx = context.lock().await;
            ctx.run_command(None, command)?;
        }

        if let Some(mask) = self.masked {
            let name = self
                .name
                .as_ref()
                .ok_or_eyre("systemd service name is required")?;

            let mut command = vec!["systemctl", scope_flag];
            if mask {
                command.push("mask");
            } else {
                command.push("unmask");
            }

            if self.force.unwrap_or_default() {
                command.push("--force");
            }

            command.push(name.as_str());

            let ctx = context.lock().await;
            ctx.run_command(None, command)?;
        }

        if let Some(state) = self.state.as_ref() {
            let name = self
                .name
                .as_ref()
                .ok_or_eyre("systemd service name is required")?;

            let mut command = vec!["systemctl", scope_flag];

            command.push(match state {
                State::Reloaded => "reload",
                State::Restarted => "restart",
                State::Started => "start",
                State::Stopped => "stop",
            });

            if self.no_block.unwrap_or_default() {
                command.push("--no-block");
            }

            command.push(name);

            let ctx = context.lock().await;
            ctx.run_command(None, command)?;
        }

        Ok(None)
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("ansible.builtin.systemd_service", &[
        "systemd_service",
        "ansible.builtin.systemd",
        "systemd",
    ], &SystemdTask::run)
}
