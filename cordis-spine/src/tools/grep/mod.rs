//! Thin `grep` name wrapper. Engine lives in `cordis_base::grep`.

use cordis_base::types::ToolSpec;

pub fn spec() -> ToolSpec {
    ToolSpec {
        name: "grep".into(),
        description: cordis_base::grep::DESCRIPTION.into(),
        parameters_json: cordis_base::grep::PARAMS.into(),
    }
}

pub async fn run(call_id: &str, args: &str) -> String {
    cordis_base::grep::run(call_id, args).await
}
