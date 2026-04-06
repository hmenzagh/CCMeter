use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::ui::time_filter::TimeFilter;

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub expanded_heatmap: bool,
    #[serde(default)]
    pub time_filter: Option<TimeFilter>,
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = config_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&path, json);
        }
    }
}

fn config_path() -> PathBuf {
    let home = dirs::home_dir().unwrap_or_default();
    home.join(".config").join("ccmeter").join("config.json")
}
