use std::{
    collections::HashMap,
    fs::File,
    io::ErrorKind,
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use clap::Parser;
use serde_yaml::Value;
use tracing::{debug, info, level_filters::LevelFilter, trace};
use tracing_subscriber::EnvFilter;

pub mod serde;
pub mod task;

use crate::serde::{
    play::{Play, PlayRole},
    task::TaskDescription,
};
use crate::task::{KeroseneTaskInfo, TaskContext, TaskId};

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

fn load_yaml<T>(path: &Path) -> eyre::Result<Option<T>>
where
    T: ::serde::de::DeserializeOwned,
{
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };

    Ok(Some(serde_yaml::from_reader::<_, T>(file)?))
}

#[derive(Debug, Parser)]
struct Cli {
    /// Path to playbook
    play: PathBuf,
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

    let args = Cli::parse();

    // Load plays from the playbook
    let plays: Vec<Play> = {
        let file = File::open(&args.play)?;
        serde_yaml::from_reader(file)?
    };

    let current_dir = std::env::current_dir()?;
    let play_basedir = args.play.parent().unwrap_or(&current_dir);

    let _ = known_tasks();

    // TODO: include inventory
    for play in plays {
        info!(name = play.name(), "processing play");
        process_play(play_basedir, play).await?;
    }

    Ok(())
}

async fn process_play(basedir: &Path, play: Play) -> eyre::Result<()> {
    let ctx: TaskContext = Default::default();

    // Process pre_tasks
    if let Some(pre_tasks) = play.pre_tasks {
        process_tasks(Arc::clone(&ctx), pre_tasks, None, true).await?;
    }

    // Process roles
    if let Some(roles) = play.roles {
        for role in roles {
            let role_basedir = basedir.join("roles").join(role.name());
            process_role(Arc::clone(&ctx), &role_basedir, role).await?;
        }
    }

    // Process tasks
    if let Some(tasks) = play.tasks {
        process_tasks(Arc::clone(&ctx), tasks, None, false).await?;
    }

    // Process role & tasks handlers here
    run_handlers(Arc::clone(&ctx)).await?;

    // Process post_tasks
    if let Some(post_tasks) = play.post_tasks {
        process_tasks(Arc::clone(&ctx), post_tasks, None, true).await?;
    }

    Ok(())
}

async fn register_handlers(
    ctx: TaskContext,
    handlers: Vec<TaskDescription>,
    role: Option<&PlayRole>,
) -> eyre::Result<()> {
    for handler in handlers {
        let task_id = handler.task_id.name();
        let name = match (&role, &handler.name) {
            (Some(role), Some(name)) => format!("{} : {}", role.name(), name),
            (Some(role), None) => format!("{} : {}", role.name(), handler.task_id.name()),
            (None, Some(name)) => name.to_string(),
            (None, None) => handler.task_id.name().to_string(),
        };

        // TODO: actually register
        // TODO: Register also unprefixed names
        debug!(
            role = role.map(PlayRole::name),
            name, task_id, "registered handler"
        );
    }

    Ok(())
}

async fn process_role(ctx: TaskContext, role_basedir: &Path, role: PlayRole) -> eyre::Result<()> {
    // TODO: handle role path

    // Load role defaults
    let role_defaults: Option<HashMap<String, Value>> =
        load_yaml(&role_basedir.join("defaults/main.yml"))?;
    if let Some(defaults) = role_defaults {
        let mut ctx = ctx.lock().await;
        for (key, value) in defaults {
            ctx.facts.entry(key).or_insert(value);
        }
    }

    // Load role handlers
    let handlers: Option<Vec<TaskDescription>> =
        load_yaml(&role_basedir.join("handlers/main.yml"))?;
    if let Some(handlers) = handlers {
        register_handlers(Arc::clone(&ctx), handlers, Some(&role)).await?;
    }

    // Load role tasks
    let tasks: Option<Vec<TaskDescription>> = load_yaml(&role_basedir.join("tasks/main.yml"))?;

    if let Some(tasks) = tasks {
        process_tasks(ctx, tasks, Some(role.name().to_string()), false).await?;
    }

    Ok(())
}

async fn process_tasks(
    ctx: TaskContext,
    // TODO: include handlers
    tasks: Vec<TaskDescription>,
    role: Option<String>,
    flush_handlers: bool,
) -> eyre::Result<()> {
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

    if flush_handlers {
        run_handlers(ctx).await?;
    }

    Ok(())
}

async fn run_handlers(ctx: TaskContext) -> eyre::Result<()> {
    let mut ctx = ctx.lock().await;
    if !ctx.pending_handlers.is_empty() {
        debug!("running pending handlers");
        ctx.consume_pending_handlers()?;
    }

    Ok(())
}
