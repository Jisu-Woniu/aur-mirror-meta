use anyhow::Result;

use crate::database::DatabaseOps;

#[derive(Clone)]
pub struct AppState {
    pub db: DatabaseOps,
    pub github_token: Option<String>,
}

impl AppState {
    pub async fn new(db_path: &str, github_token: Option<String>) -> Result<Self> {
        Ok(Self {
            db: DatabaseOps::new(db_path).await?,
            github_token,
        })
    }
}
