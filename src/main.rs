use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};

use clap::Parser;
use eyre::eyre;
use kerosene::load_yaml;
use serde::task::HandlerDescription;
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

#[derive(Debug, Parser)]
struct Cli {
    /// Path to inventory file
    #[arg(long, short = 'i')]
    inventory: PathBuf,

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
    let plays: Vec<Play> = load_yaml(&args.play)?
        .ok_or_else(|| eyre!("playbook at '{:?}' could not be opened", &args.play))?;

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
    handlers: Vec<HandlerDescription>,
    role: Option<&PlayRole>,
) -> eyre::Result<()> {
    let mut ctx = ctx.lock().await;

    for handler in handlers {
        let task_id = handler.task_id.name();

        let mut handler_names = HashSet::new();
        if let Some(name) = &handler.name {
            if let Some(role) = role {
                handler_names.insert(format!("{} : {}", role.name(), name));
            }
            handler_names.insert(name.clone());
        }

        if let Some(listen) = &handler.listen {
            handler_names.insert(listen.clone());
        }

        for name in &handler_names {
            ctx.known_handlers.insert(name.clone(), handler.clone());
        }

        debug!(
            role = role.map(PlayRole::name),
            names = ?handler_names,
            task_id,
            "registered handler"
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
    let handlers: Option<Vec<HandlerDescription>> =
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

pub async fn run_handlers(context: TaskContext) -> eyre::Result<()> {
    // HACK: There's more elegant solution than this, but I don't want to
    //       spend too much time here to design this architecture here to
    //       avoid a deadlock. Just clone the pending handlers collection
    //       instead.
    let mut pending_handlers = {
        let ctx = context.lock().await;
        ctx.pending_handlers.clone()
    };

    if !pending_handlers.is_empty() {
        debug!("running pending handlers");

        while let Some(handler_name) = pending_handlers.pop_front() {
            let (run, args) = {
                let ctx = context.lock().await;
                let handler = ctx
                    .known_handlers
                    .get(handler_name.as_str())
                    .ok_or_else(|| eyre!("Handler '{}' is not declared", handler_name))?;

                let task = get_task(handler.task_id.name()).unwrap();

                (task.run, handler.args.clone())
            };

            info!(handler_name, "running handler");
            let _ = (run)(Arc::clone(&context), args).await?;
        }

        let mut ctx = context.lock().await;
        ctx.pending_handlers.clear();
    }

    Ok(())
}
