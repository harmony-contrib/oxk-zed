//! Permission metadata used by `module.json5` completion and hover.
use serde_json::Value;
use std::{collections::BTreeMap, sync::OnceLock};

#[derive(Clone, Debug)]
pub struct Permission {
    pub description: String,
    pub level: String,
    pub kind: String,
    pub grant_mode: String,
    pub start_version: u64,
    pub note: String,
}

fn text(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn load() -> BTreeMap<String, Permission> {
    let Some(permissions) = serde_json::from_str::<Value>(include_str!("../data/permissions.json"))
        .ok()
        .and_then(|value| value.get("permissions").cloned())
        .and_then(|value| value.as_object().cloned())
    else {
        return BTreeMap::new();
    };
    permissions
        .into_iter()
        .map(|(name, value)| {
            (
                name,
                Permission {
                    description: text(&value, "description"),
                    level: text(&value, "level"),
                    kind: text(&value, "type"),
                    grant_mode: text(&value, "grantMode"),
                    start_version: value
                        .get("startVersion")
                        .and_then(Value::as_u64)
                        .unwrap_or_default(),
                    note: text(&value, "note"),
                },
            )
        })
        .collect()
}

pub fn permissions() -> &'static BTreeMap<String, Permission> {
    static PERMISSIONS: OnceLock<BTreeMap<String, Permission>> = OnceLock::new();
    PERMISSIONS.get_or_init(load)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_permissions_expose_metadata() {
        let permission = permissions()
            .get("ohos.permission.INTERNET")
            .expect("INTERNET permission is bundled");
        assert!(!permission.description.is_empty());
        assert!(!permission.level.is_empty());
        assert!(!permission.grant_mode.is_empty());
        assert!(permissions().len() > 100);
    }
}
