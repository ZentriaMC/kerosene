#[derive(Debug, Deserialize)]
struct ButaneConfig {
    variant: String,
    version: String,
    storage: Option<Storage>,
}

#[derive(Debug, Deserialize)]
struct Storage {
    files: Option<Vec<File>>,
}

#[derive(Debug, Deserialize)]
struct File {
    path: String,
    contents: Option<FileContents>,
    mode: Option<i32>,
    overwrite: Option<bool>,
    user: Option<FileUser>,
    group: Option<FileGroup>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FileContents {
    Inline { inline: String },
    Local { local: String },
    Remote { source: String },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FileUser {
    Name { name: String },
    Id { id: i32 },
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FileGroup {
    Name { name: String },
    Id { id: i32 },
}

#[derive(Debug, Serialize)]
struct AnsibleTask {
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ansible.builtin.get_url")]
    get_url: Option<HashMap<String, serde_yaml::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "ansible.builtin.copy")]
    copy: Option<HashMap<String, serde_yaml::Value>>,
}

fn is_valid_url(s: &str) -> bool {
    if s.starts_with("data:") {
        return false;
    }
    Url::parse(s).is_ok()
}

fn create_common_props(file: &File) -> HashMap<String, serde_yaml::Value> {
    let mut props = HashMap::new();

    if let Some(mode) = file.mode {
        props.insert(
            "mode".to_string(),
            serde_yaml::Value::String(format!("{:#o}", mode)),
        );
    }

    if let Some(user) = &file.user {
        match user {
            FileUser::Name { name } => {
                props.insert("owner".to_string(), serde_yaml::Value::String(name.clone()));
            }
            FileUser::Id { id } => {
                props.insert("owner".to_string(), serde_yaml::Value::Number((*id).into()));
            }
        }
    }

    if let Some(group) = &file.group {
        match group {
            FileGroup::Name { name } => {
                props.insert("group".to_string(), serde_yaml::Value::String(name.clone()));
            }
            FileGroup::Id { id } => {
                props.insert("group".to_string(), serde_yaml::Value::Number((*id).into()));
            }
        }
    }

    if let Some(overwrite) = file.overwrite {
        props.insert("force".to_string(), serde_yaml::Value::Bool(overwrite));
    }

    props
}

fn convert_file_to_task(file: &File) -> AnsibleTask {
    let mut task = AnsibleTask {
        name: format!("Manage file {}", file.path),
        get_url: None,
        copy: None,
    };

    let mut props = create_common_props(file);
    props.insert(
        "dest".to_string(),
        serde_yaml::Value::String(file.path.clone()),
    );

    if let Some(contents) = &file.contents {
        match contents {
            FileContents::Remote { source } => {
                if is_valid_url(source) {
                    let mut get_url_props = props.clone();
                    get_url_props
                        .insert("url".to_string(), serde_yaml::Value::String(source.clone()));
                    task.get_url = Some(get_url_props);
                } else if source.starts_with("data:") {
                    if let Some(content) = source.split(',').nth(1) {
                        let mut copy_props = props.clone();
                        copy_props.insert(
                            "content".to_string(),
                            serde_yaml::Value::String(content.to_string()),
                        );
                        task.copy = Some(copy_props);
                    }
                }
            }
            FileContents::Local { local } => {
                let mut copy_props = props.clone();
                copy_props.insert("src".to_string(), serde_yaml::Value::String(local.clone()));
                task.copy = Some(copy_props);
            }
            FileContents::Inline { inline } => {
                let mut copy_props = props.clone();
                copy_props.insert(
                    "content".to_string(),
                    serde_yaml::Value::String(inline.clone()),
                );
                task.copy = Some(copy_props);
            }
        }
    }

    task
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: {} <butane-config.yaml>", args[0]);
        std::process::exit(1);
    }

    for task in inventory::iter::<self::task::KeroseneTaskInfo> {
        trace!(task.fqdn, "registered task");
    }

    let config_str = fs::read_to_string(&args[1])?;
    let config: ButaneConfig = serde_yaml::from_str(&config_str)?;

    // Validate variant and version
    if !config.variant.starts_with("fcos") {
        eprintln!(
            "Warning: Unsupported variant: {}. Only FCOS variants are fully tested.",
            config.variant
        );
    }

    if let Some(storage) = config.storage {
        if let Some(files) = storage.files {
            let tasks: Vec<AnsibleTask> = files.iter().map(convert_file_to_task).collect();

            println!("---");
            println!("{}", serde_yaml::to_string(&tasks)?);
        }
    }

    Ok(())
}
