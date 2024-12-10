pub mod task;

#[derive(Clone, Debug)]
pub enum TaskId {
    Task(&'static str),
    Unknown(&'static str),
    Alias {
        id: &'static str,
        alias: &'static str,
    },
}

impl TaskId {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Task(id) => id,
            Self::Unknown(id) => id,
            Self::Alias { id, .. } => id,
        }
    }
}
