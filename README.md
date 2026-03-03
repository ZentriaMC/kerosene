# Kerosene

Ansible-compatible provisioning that requires nothing on the remote host.

## Why

Ansible requires Python on every target machine. This makes it incompatible with minimal and immutable operating systems like Fedora CoreOS, Flatcar, or Talos Linux out of the box. Kerosene solves this by executing everything over SSH using only commands from GNU coreutils and systemd (`install`, `systemctl`, `sh -c`, etc.) already present on any modern Linux distribution. No agent, no Python, no runtime on the remote.

## Features

- **SSH-native execution** -- all commands run over SSH, no remote agent needed
- **Ansible-compatible playbooks** -- reuse your existing YAML playbooks and inventory
- **Jinja2 templating** -- variable interpolation in tasks and template files via MiniJinja
- **SSH ControlMaster** -- automatic connection multiplexing (`ControlPersist=60s`)
- **Privilege escalation** -- `become` / `become_user` via sudo
- **Handlers** -- `notify` / `listen` with automatic flush at end of play
- **Roles** -- standard `roles/<name>/{tasks,handlers,defaults,files,templates}/` layout
- **Safe shell quoting** -- all remote commands are shell-quoted via `shlex`

## Usage

```
kerosene -i inventory.yml playbook.yml
```

Logging is controlled via `RUST_LOG` (defaults to TRACE):

```
RUST_LOG=info kerosene -i inventory.yml playbook.yml
```

## Playbook format

Standard Ansible YAML playbooks:

```yaml
- name: "Deploy application"
  hosts: "webservers"
  remote_user: "deploy"
  roles:
    - nginx
  pre_tasks:
    - name: "Check connectivity"
      shell:
        cmd: "uptime"
  tasks:
    - name: "Install config"
      become: true
      template:
        src: "app.conf.j2"
        dest: "/etc/app.conf"
        mode: "0644"
      notify: "restart app"
    - name: "Capture hostname"
      shell:
        cmd: "hostname"
      register: "hostname_result"
  handlers:
    - name: "restart app"
      become: true
      systemd_service:
        name: "app"
        state: restarted
```

## Inventory format

Ansible-compatible YAML inventory with host variables:

```yaml
all:
  hosts:
    webserver1:
      ansible_host: 192.168.1.10
      ansible_user: deploy
      ansible_port: 22
      ansible_ssh_private_key_file: ~/.ssh/id_ed25519
      ansible_ssh_extra_args: "-o StrictHostKeyChecking=no"
    webserver2:
      ansible_host: 192.168.1.11
```

Hosts can be targeted by group name or `all`. Connection variables follow Ansible naming (`ansible_host`, `ansible_user`, `ansible_port`, `ansible_ssh_private_key_file`, `ansible_ssh_extra_args`).

## Supported tasks

| Module | Aliases | Description |
|--------|---------|-------------|
| `ansible.builtin.shell` | `shell` | Execute shell commands via `/bin/sh -c` with optional `chdir` and `executable` |
| `ansible.builtin.copy` | `copy` | Copy files or inline content to remote, with `owner`/`group`/`mode` via `install(1)` |
| `ansible.builtin.template` | `template` | Render Jinja2 templates and deploy to remote, with `owner`/`group`/`mode` |
| `ansible.builtin.systemd_service` | `systemd_service`, `systemd` | Manage systemd units: start/stop/restart/reload, enable/disable, daemon-reload, mask |
| `ansible.builtin.set_fact` | `set_fact` | Set variables (facts) that persist for the rest of the play |
| `ansible.builtin.meta` | `meta` | Control play execution: `flush_handlers`, `reset_connection`, `noop` |
| `kerosene.builtin.curl` | `curl` | Execute curl requests on the remote with optional method and headers |
| `ansible.builtin.import_tasks` | `import_tasks` | Stub (not yet implemented) |

## Variable precedence

Variables are resolved in three layers, lowest to highest precedence:

1. **Role defaults** -- `roles/<name>/defaults/main.yml`, scoped per role
2. **Facts** -- set via `set_fact`, persists across the entire play
3. **Role play vars** -- `vars:` on the role entry in the play, scoped per role

Higher layers override lower layers. All variables are available in Jinja2 expressions for task arguments and template rendering.

## Role structure

```
roles/
  my_role/
    tasks/main.yml
    handlers/main.yml
    defaults/main.yml
    files/
    templates/
```

File resolution for `copy` and `template` tasks searches the role's directory first, then falls back to the playbook's base directory.

## Development

### Prerequisites

Dependencies are managed via Nix:

```
nix develop
```

This provides `butane`, `jq`, and `qemu` for development and E2E testing.

### Building

```
cargo build --release
```

### E2E tests

The test suite boots a Fedora CoreOS VM with QEMU, validates it with goss, then runs a kerosene playbook against it:

```
hack/test.sh
```

The VM runs in snapshot mode (ephemeral) and is cleaned up on exit. Set `KEEP_VM=1` to keep it running for debugging.

### CI

GitHub Actions runs `cargo fmt --check` and `cargo clippy` on pushes and PRs to master.

## Current limitations

- `when` conditionals are parsed but not evaluated
- `register` captures output but templating support is basic
- `vars` on tasks are parsed but not injected
- `delegate_to` only supports `localhost`
- `import_tasks` is a stub (no-op)
- Remote template sources (`remote_src: true`) are not implemented
- Inventory patterns only support `all` or a single group name (no glob/regex)
- No `--check` (dry run) mode exposed via CLI
- No `changed` / `ok` / `failed` status tracking per task
