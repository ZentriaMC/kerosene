use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::OnceLock,
};

use clap::Parser;
use command::CommandTarget;
use eyre::eyre;
use kerosene::load_yaml;
use serde::task::HandlerDescription;
use serde_yaml::Value;
use tracing::{debug, info, level_filters::LevelFilter, trace, warn};
use tracing_subscriber::EnvFilter;

pub mod command;
pub mod inventory;
pub mod render;
pub mod serde;
pub mod task;

use crate::inventory::{Inventory, is_localhost};
use crate::serde::{
    play::{Play, PlayRole},
    task::TaskDescription,
};
use crate::task::{KeroseneTaskInfo, TaskContext, TaskId};

#[derive(Debug, Default)]
struct PlayStats {
    ok: usize,
    changed: usize,
    failed: usize,
}

impl std::ops::AddAssign for PlayStats {
    fn add_assign(&mut self, rhs: Self) {
        self.ok += rhs.ok;
        self.changed += rhs.changed;
        self.failed += rhs.failed;
    }
}

pub fn known_tasks() -> &'static HashMap<&'static str, TaskId> {
    static TASKS: OnceLock<HashMap<&'static str, TaskId>> = OnceLock::new();

    TASKS.get_or_init(|| {
        let mut all_tasks = HashMap::new();
        for task in ::inventory::iter::<self::task::KeroseneTaskInfo> {
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
        for task in ::inventory::iter::<self::task::KeroseneTaskInfo> {
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

    // Load inventory
    let inv: Inventory = load_yaml(&args.inventory)?
        .ok_or_else(|| eyre!("inventory at '{:?}' could not be opened", &args.inventory))?;

    let mut host_stats: HashMap<String, PlayStats> = HashMap::new();

    for play in plays {
        let hosts = inv.resolve_hosts(&play.hosts)?;

        for host in &hosts {
            info!(name = play.name(), host = host.name, "processing play");

            let command_target = if is_localhost(host) {
                CommandTarget::Local {
                    elevate: None,
                    dry: false,
                }
            } else {
                CommandTarget::Remote {
                    hostname: host.hostname.clone(),
                    user: host.user.clone().or(play.remote_user.clone()),
                    port: host.port,
                    ssh_key: host.ssh_key.clone(),
                    ssh_extra_args: host.ssh_extra_args.clone(),
                    elevate: None,
                    dry: false,
                }
            };

            let result = process_play(play_basedir, play.clone(), command_target.clone()).await;
            command_target.reset().await?;
            let stats = result?;
            *host_stats.entry(host.name.clone()).or_default() += stats;
        }
    }

    // Play recap
    for (host, stats) in &host_stats {
        info!(
            host,
            ok = stats.ok + stats.changed,
            changed = stats.changed,
            failed = stats.failed,
            "play recap",
        );
    }

    Ok(())
}

async fn process_play(
    basedir: &Path,
    play: Play,
    command_target: CommandTarget,
) -> eyre::Result<PlayStats> {
    let ctx: TaskContext = TaskContext::new(basedir.to_path_buf());
    ctx.lock().await.command_target = command_target;
    let mut stats = PlayStats::default();

    // Process pre_tasks
    if let Some(pre_tasks) = play.pre_tasks {
        stats += process_tasks(ctx.clone(), pre_tasks, None, true).await?;
    }

    // Process roles
    if let Some(roles) = play.roles {
        for role in roles {
            let role_basedir = basedir.join("roles").join(role.name());
            ctx.lock()
                .await
                .resource_dirs
                .push_front(role_basedir.clone());

            let result = process_role(ctx.clone(), &role_basedir, role).await;

            // Always clean up role-scoped state, even on error
            {
                let mut ctx_inner = ctx.lock().await;
                ctx_inner.resource_dirs.pop_front();
                ctx_inner.role_defaults.clear();
                ctx_inner.role_play_vars.clear();
            }

            stats += result?;
        }
    }

    // Process tasks
    if let Some(tasks) = play.tasks {
        stats += process_tasks(ctx.clone(), tasks, None, false).await?;
    }

    // Process role & tasks handlers
    run_handlers(ctx.clone()).await?;

    // Process post_tasks
    if let Some(post_tasks) = play.post_tasks {
        stats += process_tasks(ctx.clone(), post_tasks, None, true).await?;
    }

    Ok(stats)
}

async fn register_handlers(
    ctx: TaskContext,
    handlers: Vec<HandlerDescription>,
    role: Option<&PlayRole>,
    role_resource_dir: Option<PathBuf>,
) -> eyre::Result<()> {
    let mut ctx = ctx.lock().await;

    for mut handler in handlers {
        handler.role_resource_dir = role_resource_dir.clone();
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

async fn process_role(
    ctx: TaskContext,
    role_basedir: &Path,
    role: PlayRole,
) -> eyre::Result<PlayStats> {
    // TODO: handle role path

    // Load role defaults into scoped role_defaults (not persistent facts)
    let defaults: Option<HashMap<String, Value>> =
        load_yaml(&role_basedir.join("defaults/main.yml"))?;
    if let Some(defaults) = defaults {
        ctx.lock().await.role_defaults = defaults;
    }

    // Inject play-level role vars into scoped role_play_vars
    if let Some(vars) = role.vars() {
        ctx.lock().await.role_play_vars = vars.clone();
    }

    // Load role handlers
    let handlers: Option<Vec<HandlerDescription>> =
        load_yaml(&role_basedir.join("handlers/main.yml"))?;
    if let Some(handlers) = handlers {
        register_handlers(
            ctx.clone(),
            handlers,
            Some(&role),
            Some(role_basedir.to_path_buf()),
        )
        .await?;
    }

    // Load role tasks
    let tasks: Option<Vec<TaskDescription>> = load_yaml(&role_basedir.join("tasks/main.yml"))?;

    if let Some(tasks) = tasks {
        return process_tasks(ctx, tasks, Some(role.name().to_string()), false).await;
    }

    Ok(PlayStats::default())
}

async fn process_tasks(
    ctx: TaskContext,
    tasks: Vec<TaskDescription>,
    role: Option<String>,
    flush_handlers: bool,
) -> eyre::Result<PlayStats> {
    let mut stats = PlayStats::default();

    for task in tasks {
        let task_id = task.task_id.name();
        let name = match (&role, &task.name) {
            (Some(role), Some(name)) => format!("{role} : {name}"),
            (Some(role), None) => format!("{role} : {}", task.task_id.name()),
            (None, Some(name)) => name.to_string(),
            (None, None) => task.task_id.name().to_string(),
        };

        if let Some(role) = &role {
            info!(role, name, task_id, "running task");
        } else {
            info!(name, task_id, "running task");
        }
        let task_info = get_task(task_id).unwrap();
        let ignore_errors = task.ignore_errors;
        ctx.lock().await.do_become_user = if task.r#become {
            Some(task.become_user.unwrap_or("root".to_string()))
        } else {
            None
        };

        let prev_command_target: Option<CommandTarget> = if let Some(delegate_to) = task.delegate_to
        {
            if delegate_to == "localhost" || delegate_to == "127.0.0.1" {
                let mut ctx_inner = ctx.lock().await;
                Some(std::mem::replace(
                    &mut ctx_inner.command_target,
                    // TODO
                    CommandTarget::Local {
                        elevate: None,
                        dry: false,
                    },
                ))
            } else {
                None
            }
        } else {
            None
        };

        ctx.lock().await.task_vars = task.vars.unwrap_or_default();

        let resolved_vars = render::resolve_vars(&ctx.lock().await.merged_vars())?;
        let rendered_args = render::render_value(&task.args, &resolved_vars)?;

        match (task_info.run)(ctx.clone(), rendered_args).await {
            Ok(result) => {
                if result.changed {
                    stats.changed += 1;
                    info!(name, "changed");
                } else {
                    stats.ok += 1;
                    info!(name, "ok");
                }

                if let (Some(register), Some(mut value)) = (&task.register, result.output) {
                    // Inject `changed` key into registered output mapping
                    if let Value::Mapping(ref mut map) = value {
                        map.insert(Value::String("changed".into()), Value::Bool(result.changed));
                    }
                    debug!(register, "registering output");
                    ctx.lock().await.facts.insert(register.clone(), value);
                }

                if result.changed {
                    for notify in task.notify {
                        let rendered_notify = render::render_str(&notify, &resolved_vars)?;
                        let mut ctx = ctx.lock().await;
                        ctx.pending_handlers.push_back(rendered_notify);
                    }
                }
            }
            Err(err) => {
                stats.failed += 1;
                if ignore_errors {
                    warn!(name, ?err, "failed (ignored)");
                } else {
                    // Clean up before returning
                    ctx.lock().await.task_vars.clear();
                    if let Some(command_target) = prev_command_target {
                        ctx.lock().await.command_target = command_target;
                    }
                    return Err(err);
                }
            }
        }

        ctx.lock().await.task_vars.clear();

        if let Some(command_target) = prev_command_target {
            let mut ctx_inner = ctx.lock().await;
            ctx_inner.command_target = command_target;
        }
    }

    if flush_handlers {
        run_handlers(ctx).await?;
    }

    Ok(stats)
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
            let (run, args, become_user, role_resource_dir, handler_vars) = {
                let ctx = context.lock().await;
                let handler = ctx
                    .known_handlers
                    .get(handler_name.as_str())
                    .ok_or_else(|| eyre!("Handler '{}' is not declared", handler_name))?;

                let become_user = if handler.r#become {
                    Some(handler.become_user.clone().unwrap_or("root".to_string()))
                } else {
                    None
                };

                let task = get_task(handler.task_id.name()).unwrap();
                (
                    task.run,
                    handler.args.clone(),
                    become_user,
                    handler.role_resource_dir.clone(),
                    handler.vars.clone().unwrap_or_default(),
                )
            };

            // Push handler's role resource dir so it can resolve role-local files
            if let Some(ref dir) = role_resource_dir {
                context.lock().await.resource_dirs.push_front(dir.clone());
            }

            info!(handler_name, "running handler");
            {
                let mut ctx = context.lock().await;
                ctx.do_become_user = become_user;
                ctx.task_vars = handler_vars;
            }
            let result = (run)(context.clone(), args).await;

            // Always clean up resource dir and task vars, even on error
            {
                let mut ctx = context.lock().await;
                ctx.task_vars.clear();
                if role_resource_dir.is_some() {
                    ctx.resource_dirs.pop_front();
                }
            }

            let _ = result?;
        }

        let mut ctx = context.lock().await;
        ctx.pending_handlers.clear();
    }

    Ok(())
}
