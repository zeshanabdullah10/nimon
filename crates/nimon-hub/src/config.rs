//! Hub server configuration

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HubConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_db_path")]
    pub database_path: String,
    #[serde(default)]
    pub alert: AlertConfig,
}

fn default_host() -> String { "0.0.0.0".to_string() }
fn default_port() -> u16 { 8080 }
fn default_db_path() -> String { "data/nimon.db".to_string() }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertConfig {
    #[serde(default = "default_cooldown")]
    pub default_cooldown_minutes: i64,
    #[serde(default = "default_max_firing")]
    pub max_firing_count: i32,
}

fn default_cooldown() -> i64 { 5 }
fn default_max_firing() -> i32 { 100 }

impl Default for AlertConfig {
    fn default() -> Self {
        Self { default_cooldown_minutes: default_cooldown(), max_firing_count: default_max_firing() }
    }
}

impl Default for HubConfig {
    fn default() -> Self {
        Self { host: default_host(), port: default_port(), database_path: default_db_path(), alert: AlertConfig::default() }
    }
}

impl HubConfig {
    pub fn from_file(path: &Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config: HubConfig = serde_yaml::from_str(&content)?;
        Ok(config)
    }

    pub fn load_or_default(path: &Path) -> anyhow::Result<Self> {
        if path.exists() {
            Self::from_file(path)
        } else {
            Ok(Self::default())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HubConfig::default();
        assert_eq!(config.host, "0.0.0.0");
        assert_eq!(config.port, 8080);
        assert_eq!(config.database_path, "data/nimon.db");
    }

    #[test]
    fn test_from_yaml() {
        let yaml = "port: 9090\ndatabase_path: /tmp/nimon.db";
        let config: HubConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.port, 9090);
        assert_eq!(config.database_path, "/tmp/nimon.db");
    }
}
