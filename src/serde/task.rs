use std::collections::HashMap;

use serde::{Deserialize, Deserializer};
use serde_yaml::Value;
use tracing::debug;

use crate::{known_tasks, task::TaskId};

#[derive(Clone, Debug)]
pub struct TaskDescription {
    pub name: Option<String>,
    pub task_id: TaskId,
    pub args: Value,
    pub r#become: bool,
    pub become_user: Option<String>,
    pub delegate_to: Option<String>,

    pub when: Vec<String>,
    pub notify: Vec<String>,
    pub register: Option<String>,
    pub vars: Option<HashMap<String, Value>>,
}

impl<'de> Deserialize<'de> for TaskDescription {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let result = deserializer.deserialize_struct(
            "task",
            &[],
            TaskVisitor {
                expect_handler: false,
            },
        )?;

        match result {
            TaskOrHandler::Task(task) => Ok(task),
            _ => unreachable!(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct HandlerDescription {
    pub name: Option<String>,
    pub task_id: TaskId,
    pub args: Value,
    pub r#become: bool,
    pub become_user: Option<String>,

    pub when: Vec<String>,
    pub listen: Option<String>,
    pub vars: Option<HashMap<String, Value>>,
}

impl<'de> Deserialize<'de> for HandlerDescription {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let result = deserializer.deserialize_struct(
            "handler",
            &[],
            TaskVisitor {
                expect_handler: true,
            },
        )?;

        match result {
            TaskOrHandler::Handler(handler) => Ok(handler),
            _ => unreachable!(),
        }
    }
}

enum TaskOrHandler {
    Task(TaskDescription),
    Handler(HandlerDescription),
}

struct TaskVisitor {
    expect_handler: bool,
}

impl<'de> serde::de::Visitor<'de> for TaskVisitor {
    type Value = TaskOrHandler;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        if self.expect_handler {
            write!(formatter, "handler")
        } else {
            write!(formatter, "task")
        }
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::MapAccess<'de>,
    {
        // strike! {
        //     #[strikethrough[derive(Debug, Deserialize)]]
        //     struct TaskDescriptionCommon {
        //         #[serde(default)]
        //         name: Option<String>,
        //         #[serde(default)]
        //         r#become: Option<bool>,
        //         #[serde(default)]
        //         when: Option<#[serde(untagged)] enum {
        //             Expr(String),
        //             Exprs(Vec<String>)
        //         }>,
        //         #[serde(default)]
        //         notify: Option<Vec<String>>,

        //         #[serde(default, flatten)]
        //         other: HashMap<String, Value>,
        //     }
        // }

        let mut name = None::<String>;
        let mut task_id = None::<TaskId>;
        let mut args = None::<Value>;
        let mut r#become = None::<bool>;
        let mut become_user = None::<String>;
        let mut delegate_to = None::<String>;
        let mut when = None::<Vec<String>>;
        let mut notify = None::<Vec<String>>;
        let mut register = None::<String>;
        let mut vars = None::<HashMap<String, Value>>;
        let mut listen = None::<String>;

        while let Some((key, value)) = map.next_entry::<String, Value>()? {
            match key.as_str() {
                "name" => {
                    if name.is_none() {
                        name = Some(
                            value
                                .as_str()
                                .map(str::to_owned)
                                .ok_or(serde::de::Error::custom("name is not a string"))?,
                        );
                    } else {
                        return Err(serde::de::Error::custom("duplicate name"));
                    }
                }
                "delegate_to" => {
                    if delegate_to.is_none() {
                        delegate_to = Some(
                            value
                                .as_str()
                                .map(str::to_owned)
                                .ok_or(serde::de::Error::custom("delegate_to is not a string"))?,
                        );
                    } else {
                        return Err(serde::de::Error::custom("duplicate delegate_to"));
                    }
                }
                "become" => {
                    if r#become.is_none() {
                        r#become = Some(
                            value
                                .as_bool()
                                .ok_or(serde::de::Error::custom("name is not a boolean"))?,
                        );
                    } else {
                        return Err(serde::de::Error::custom("duplicate become"));
                    }
                }
                "become_user" => {
                    if become_user.is_none() {
                        become_user = Some(
                            value
                                .as_str()
                                .map(str::to_owned)
                                .ok_or(serde::de::Error::custom("become_user is not a string"))?,
                        );
                    } else {
                        return Err(serde::de::Error::custom("duplicate become_user"));
                    }
                }
                "when" => {
                    if when.is_none() {
                        if let Some(expr) = value.as_str() {
                            when = Some(vec![expr.to_owned()]);
                        } else if value.as_sequence().is_some() {
                            let parsed = match serde_yaml::from_value(value) {
                                Err(_) => {
                                    return Err(serde::de::Error::custom(
                                        "expected when to be a list of strings",
                                    ))
                                }
                                Ok(v) => v,
                            };

                            when = Some(parsed);
                        } else {
                            return Err(serde::de::Error::custom(
                                "expected when to be a list of strings, or a string",
                            ));
                        }
                    } else {
                        return Err(serde::de::Error::custom("duplicate when"));
                    }
                }
                "listen" if self.expect_handler => {
                    if listen.is_none() {
                        listen = Some(
                            value
                                .as_str()
                                .map(str::to_owned)
                                .ok_or(serde::de::Error::custom("listen is not a string"))?,
                        );
                    } else {
                        return Err(serde::de::Error::custom("duplicate listen"));
                    }
                }
                "notify" if !self.expect_handler => {
                    if notify.is_none() {
                        let parsed = match serde_yaml::from_value(value) {
                            Err(_) => {
                                return Err(serde::de::Error::custom(
                                    "expected notify to be a list of strings",
                                ))
                            }
                            Ok(v) => v,
                        };

                        notify = Some(parsed);
                    } else {
                        return Err(serde::de::Error::custom("duplicate notify"));
                    }
                }
                "register" => {
                    if register.is_none() {
                        register = Some(
                            value
                                .as_str()
                                .map(str::to_owned)
                                .ok_or(serde::de::Error::custom("register is not a string"))?,
                        );
                    } else {
                        return Err(serde::de::Error::custom("duplicate register"));
                    }
                }
                "vars" => {
                    if vars.is_none() {
                        vars = Some(
                            HashMap::<String, Value>::deserialize(value)
                                .map_err(|_| serde::de::Error::custom("vars is not a mapping"))?,
                        )
                    } else {
                        return Err(serde::de::Error::custom("duplicate vars"));
                    }
                }
                key => {
                    if let Some(task) = known_tasks().get(key) {
                        if task_id.is_none() {
                            task_id = Some(task.clone());
                            args = Some(value);
                        } else {
                            return Err(serde::de::Error::custom("duplicate task details"));
                        }
                    } else {
                        debug!(key, "unhandled task key")
                    }
                }
            }
        }

        let _ = listen;
        Ok(if self.expect_handler {
            if name.is_none() && listen.is_none() {
                return Err(serde::de::Error::custom(
                    "handler is expected to have at least name or listen",
                ));
            }

            TaskOrHandler::Handler(HandlerDescription {
                name,
                task_id: task_id.unwrap(),
                args: args.unwrap(),
                r#become: r#become.unwrap_or_default(),
                become_user,
                when: when.unwrap_or_default(),
                listen,
                vars,
            })
        } else {
            TaskOrHandler::Task(TaskDescription {
                name,
                task_id: task_id.unwrap(),
                args: args.unwrap(),
                r#become: r#become.unwrap_or_default(),
                become_user,
                delegate_to,
                when: when.unwrap_or_default(),
                notify: notify.unwrap_or_default(),
                register,
                vars,
            })
        })
    }
}
