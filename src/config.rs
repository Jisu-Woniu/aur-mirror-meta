use std::{
    env,
    path::{Path, PathBuf},
};

use anyhow::{Result, anyhow};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct ConfigFileModel {
    pub db_path: Option<String>,
    pub github_token: Option<String>,
}

pub struct Config {
    config_path: Option<PathBuf>,
}

impl Config {
    pub fn new(config_path: Option<PathBuf>) -> Self {
        Config {
            config_path: config_path.or_else(get_default_config_path),
        }
    }

    pub fn config_path(&self) -> Option<&Path> {
        self.config_path.as_deref()
    }

    fn read_from_file(&self) -> Option<ConfigFileModel> {
        self.config_path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|content| toml::from_str::<ConfigFileModel>(&content).ok())
    }

    pub fn modify_file<M>(&self, modifier: M) -> Result<()>
    where
        M: FnOnce(&mut ConfigFileModel),
    {
        let config_path = self
            .config_path
            .as_deref()
            .ok_or(anyhow!("No config path found."))?;
        let mut model = self.read_from_file().unwrap_or_default();
        modifier(&mut model);
        let toml_str = toml::to_string_pretty(&model)?;
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(config_path, toml_str)?;
        Ok(())
    }

    pub fn db_path(&self) -> Option<String> {
        self.read_from_file()
            .and_then(|config| config.db_path)
            .or_else(|| env::var("AMM_DB_PATH").ok())
            .or_else(|| get_default_db_path().map(|p| p.to_string_lossy().to_string()))
            .filter(|path| {
                PathBuf::from(path)
                    .parent()
                    .and_then(|p| std::fs::create_dir_all(p).ok())
                    .is_some()
            })
    }

    pub fn github_token(&self) -> Option<String> {
        self.read_from_file()
            .and_then(|config| config.github_token)
            .or_else(|| env::var("AMM_GITHUB_TOKEN").ok())
            .or_else(|| env::var("GITHUB_TOKEN").ok())
    }
}

fn get_default_config_path() -> Option<PathBuf> {
    dirs::config_dir().map(|mut path| {
        path.push("aur-mirror-meta");
        path.push("config.toml");
        path
    })
}

fn get_default_db_path() -> Option<PathBuf> {
    dirs::data_dir().map(|mut path| {
        path.push("aur-mirror-meta");
        path.push("aur-meta.db");
        path
    })
}
