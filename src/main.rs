use std::{
    collections::HashMap,
    sync::{Arc, OnceLock},
};

use minijinja::context;
use tokio::sync::Mutex;
use tracing::{debug, info, level_filters::LevelFilter, trace};
use tracing_subscriber::EnvFilter;

pub mod serde;
pub mod task;

use crate::serde::{task::TaskDescription, TaskId};
use crate::task::{KeroseneTaskInfo, TaskContext};

pub fn known_tasks() -> &'static HashMap<&'static str, TaskId> {
    static TASKS: OnceLock<HashMap<&'static str, TaskId>> = OnceLock::new();

    TASKS.get_or_init(|| {
        let mut all_tasks = HashMap::new();
        for task in inventory::iter::<self::task::KeroseneTaskInfo> {
            all_tasks.insert(task.fqdn, TaskId::Task(task.fqdn));
            if let Some(aliases) = task.aliases {
                for alias in aliases {
                    all_tasks.insert(
                        alias,
                        TaskId::Alias {
                            alias,
                            id: task.fqdn,
                        },
                    );
                }
            }

            trace!(task.fqdn, ?task.aliases, "registered task");
        }
        all_tasks
    })
}

pub fn get_task(id: &'static str) -> Option<&'static KeroseneTaskInfo> {
    if let Some(task_id) = known_tasks().get(id) {
        for task in inventory::iter::<self::task::KeroseneTaskInfo> {
            if task.fqdn == task_id.name() {
                return Some(task);
            }
        }
    }

    None
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::builder()
                // .with_default_directive(LevelFilter::INFO.into())
                .with_default_directive(LevelFilter::TRACE.into())
                .from_env_lossy(),
        )
        .with_writer(std::io::stderr)
        .init();

    let handlers_example = r#"
---
- name: "Do systemd daemon reload"
  become: true
  ansible.builtin.systemd:
    daemon_reload: true
  listen: "protocol-solana : Do systemd daemon reload"

- name: "Restart Solana service"
  become: true
  ansible.builtin.systemd:
    name: "solana"
    state: "restarted"
  listen: "protocol-solana : Restart Solana service"
"#;

    let tasks_example = r#"
---
- name: "Set validator binary name fact"
  ansible.builtin.set_fact:
    validator_binary_name: "agave-validator-jito"
  when: >
    solana_use_jito

- name: "Write Solana systemd service"
  become: true
  ansible.builtin.template:
    src: "solana.service.j2"
    dest: "/etc/systemd/system/solana.service"
    owner: "root"
    group: "root"
    mode: "0644"
  notify:
    - "protocol-solana : Do systemd daemon reload"

- name: "Enable Solana systemd service"
  become: true
  ansible.builtin.systemd:
    name: "solana"
    enabled: true
  notify:
    - "protocol-solana : Restart solana service"

- name: "Flush handlers"
  ansible.builtin.meta: "flush_handlers"
"#;

    let mut ctx: TaskContext = Default::default();
    let role = None::<String>;

    let handlers: Vec<TaskDescription> = serde_yaml::from_str(handlers_example)?;
    for handler in handlers {
        let task_id = handler.task_id.name();
        let name = match (&role, &handler.name) {
            (Some(role), Some(name)) => format!("{role} : {name}"),
            (Some(role), None) => format!("{role} : {}", handler.task_id.name()),
            (None, Some(name)) => name.to_string(),
            (None, None) => handler.task_id.name().to_string(),
        };

        debug!(?role, name, task_id, "registered handler");
    }

    let tasks: Vec<TaskDescription> = serde_yaml::from_str(tasks_example)?;
    for task in tasks {
        let task_id = task.task_id.name();
        let name = match (&role, &task.name) {
            (Some(role), Some(name)) => format!("{role} : {name}"),
            (Some(role), None) => format!("{role} : {}", task.task_id.name()),
            (None, Some(name)) => name.to_string(),
            (None, None) => task.task_id.name().to_string(),
        };

        info!(?role, name, task_id, "running task");
        let task_info = get_task(task_id).unwrap();
        ctx.lock().await.do_become_user = if task.r#become {
            Some(task.become_user.unwrap_or("root".to_string()))
        } else {
            None
        };

        let _ = (task_info.run)(Arc::clone(&ctx), task.args.clone()).await?;
        for notify in task.notify {
            let mut ctx = ctx.lock().await;
            ctx.pending_handlers.push_back(notify);
        }
    }

    {
        let mut ctx = ctx.lock().await;
        if !ctx.pending_handlers.is_empty() {
            debug!("running pending handlers");
            ctx.consume_pending_handlers()?;
        }
    }

    Ok(())
}
