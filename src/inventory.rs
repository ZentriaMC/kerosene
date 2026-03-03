use std::collections::HashMap;

use eyre::eyre;
use serde::Deserialize;
use serde_yaml::Value;

/// Top-level inventory: group name → InventoryGroup
#[derive(Debug, Deserialize)]
pub struct Inventory(pub HashMap<String, InventoryGroup>);

#[derive(Debug, Deserialize)]
pub struct InventoryGroup {
    pub hosts: Option<HashMap<String, Option<HostVars>>>,
}

/// Per-host connection variables (Ansible-compatible names).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HostVars {
    pub ansible_host: Option<String>,
    pub ansible_user: Option<String>,
    pub ansible_port: Option<u16>,
    pub ansible_ssh_private_key_file: Option<String>,
    pub ansible_ssh_extra_args: Option<String>,

    #[serde(flatten)]
    pub extra: HashMap<String, Value>,
}

/// A host resolved from inventory, ready for CommandTarget construction.
#[derive(Debug, Clone)]
pub struct ResolvedHost {
    pub name: String,
    pub hostname: String,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub ssh_key: Option<String>,
    pub ssh_extra_args: Vec<String>,
}

impl Inventory {
    /// Resolve a play's `hosts:` pattern to a list of hosts.
    /// Supports "all" (every host in every group) or a single group name.
    pub fn resolve_hosts(&self, pattern: &str) -> eyre::Result<Vec<ResolvedHost>> {
        let groups: Vec<&InventoryGroup> = if pattern == "all" {
            self.0.values().collect()
        } else if let Some(group) = self.0.get(pattern) {
            vec![group]
        } else {
            return Err(eyre!("no group matched pattern '{pattern}'"));
        };

        let mut resolved = Vec::new();
        for group in groups {
            if let Some(hosts) = &group.hosts {
                for (name, vars) in hosts {
                    resolved.push(resolve_host(name, vars.as_ref()));
                }
            }
        }

        if resolved.is_empty() {
            return Err(eyre!("no hosts matched pattern '{pattern}'"));
        }

        Ok(resolved)
    }
}

fn resolve_host(name: &str, vars: Option<&HostVars>) -> ResolvedHost {
    let (hostname, user, port, ssh_key, ssh_extra_args) = match vars {
        Some(v) => (
            v.ansible_host.clone().unwrap_or_else(|| name.to_owned()),
            v.ansible_user.clone(),
            v.ansible_port,
            v.ansible_ssh_private_key_file.clone(),
            v.ansible_ssh_extra_args
                .as_deref()
                .and_then(shlex::split)
                .unwrap_or_default(),
        ),
        None => (name.to_owned(), None, None, None, Vec::new()),
    };

    ResolvedHost {
        name: name.to_owned(),
        hostname,
        user,
        port,
        ssh_key,
        ssh_extra_args,
    }
}

pub fn is_localhost(host: &ResolvedHost) -> bool {
    host.name == "localhost" || host.name == "127.0.0.1" || host.name == "::1"
}
