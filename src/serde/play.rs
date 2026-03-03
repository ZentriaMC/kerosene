use std::collections::HashMap;

use serde::Deserialize;
use serde_yaml::Value;

use super::task::TaskDescription;

#[derive(Clone, Debug, Deserialize)]
pub struct Play {
    pub name: Option<String>,
    pub hosts: String,
    pub remote_user: Option<String>,

    pub pre_tasks: Option<Vec<TaskDescription>>,
    pub roles: Option<Vec<PlayRole>>,
    pub tasks: Option<Vec<TaskDescription>>,
    pub post_tasks: Option<Vec<TaskDescription>>,
}

impl Play {
    pub fn name(&self) -> &str {
        self.name.as_deref().unwrap_or(&self.hosts)
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum PlayRole {
    RoleName(String),
    Role {
        role: String,
        vars: Option<HashMap<String, Value>>,
    },
}

impl PlayRole {
    pub fn name(&self) -> &str {
        match self {
            Self::RoleName(name) => name,
            Self::Role { role, .. } => role,
        }
    }

    pub fn vars(&self) -> Option<&HashMap<String, Value>> {
        match self {
            Self::RoleName(_) => None,
            Self::Role { vars, .. } => vars.as_ref(),
        }
    }
}
