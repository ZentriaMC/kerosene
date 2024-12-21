use std::collections::HashMap;

use async_trait::async_trait;
use serde::Deserialize;
use structstruck::strike;

use crate::task::KeroseneTaskInfo;

use super::{StructuredTask, TaskContext, TaskResult};

strike! {
    #[strikethrough[derive(Debug, Deserialize)]]
    pub struct Curl {
        pub url: String,
        pub method: Option<String>,
        pub headers: Option<HashMap<String, String>>,
    }
}

#[async_trait]
impl StructuredTask for Curl {
    async fn run_structured(&self, context: TaskContext) -> TaskResult {
        let mut command: Vec<String> = vec!["curl".into()];

        if let Some(method) = &self.method {
            command.push(format!("--request={method}"));
        }
        if let Some(headers) = &self.headers {
            for (key, value) in headers {
                command.push(format!("--header={key}: {value}"));
            }
        }

        let ctx = context.lock().await;
        ctx.run_command(None, command.iter().map(String::as_str).collect())?;

        Ok(None)
    }
}

inventory::submit! {
    KeroseneTaskInfo::new_aliases("kerosene.builtin.curl", &["curl"], &Curl::run)
}
