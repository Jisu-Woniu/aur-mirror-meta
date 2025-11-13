use std::{convert::Infallible, fmt::Write, time::Duration};

use anyhow::{Result, anyhow};
use bytes::{Buf, BufMut, BytesMut};
use chrono::{DateTime, Utc};
use reqwest::{Client, header};
use rustc_hash::FxHashMap;
use tokio::{sync::mpsc::channel, time::sleep};
use tracing::{info, warn};

use crate::types::GqlFetchSrcInfoResponse;

const AUR_GIT_UPLOAD_PACK_GET_URL: &str =
    "https://github.com/archlinux/aur.git/info/refs?service=git-upload-pack";
const GITHUB_GRAPHQL_URL: &str = "https://api.github.com/graphql";
const RETRY_AFTER_FINETUNING: u64 = 15;

#[derive(Clone)]
pub struct AurFetcher {
    client: Client,
    github_token: Option<String>,
}

impl AurFetcher {
    pub fn new(github_token: Option<String>) -> Self {
        let client = Client::new();
        Self {
            client,
            github_token,
        }
    }

    pub fn github_token(&self) -> Option<&str> {
        self.github_token.as_deref()
    }

    pub fn user_agent() -> String {
        format!("AUR-Mirror-Meta/{}", env!("CARGO_PKG_VERSION"))
    }

    pub async fn fetch_branch_list(&self) -> Result<FxHashMap<String, String>> {
        let mut request_builder = self.client.get(AUR_GIT_UPLOAD_PACK_GET_URL);
        if let Some(token) = &self.github_token {
            request_builder = request_builder.basic_auth(token, None::<Infallible>);
        }
        let mut response = request_builder.send().await?;
        if !response.status().is_success() {
            return Err(anyhow!("Failed to fetch refs: {}", response.status()));
        }

        info!("Got response, decoding...");

        let (tx, mut rx) = channel(5);
        tokio::spawn(async move {
            let mut buffer = BytesMut::new();
            while let Some(chunk) = response.chunk().await.expect("Unexpected EOF") {
                buffer.put(chunk);
                while let Some(index) = buffer.iter().position(|&b| b == b'\n') {
                    let line = buffer.split_to(index);
                    buffer.advance(1);
                    tx.send(line).await.expect("Broken pipe");
                }
            }
            tx.send(buffer).await.expect("Broken pipe")
        });

        let mut branches = FxHashMap::default();

        while let Some(line) = rx.recv().await {
            // line = "003d1671c778dfeab04b64686baf782c5baa2d96b2ec refs/heads/paru"
            //         LEN|                                   HASH|            REF|
            if let Some((commit, branch_name)) = String::from_utf8_lossy(&line)
                .trim_ascii()
                .split_once(" refs/heads/")
                && commit.len() >= 4
            {
                let commit_id = &commit[4..]; // Remove the length prefix
                if branch_name != "main" {
                    branches.insert(branch_name.to_string(), commit_id.to_string());
                }
            }
        }
        Ok(branches)
    }

    pub async fn fetch_srcinfo_batch(
        &self,
        commits: impl Iterator<Item = impl AsRef<str>>,
    ) -> Result<impl Iterator<Item = String>> {
        let mut n_commits: usize = 0;
        let mut query = String::from(r#"query{repository(owner:"archlinux",name:"aur"){"#);
        for (i, commit) in commits.enumerate() {
            write!(
                query,
                r#"x{i}:object(expression:"{}:.SRCINFO"){{... on Blob{{text}}}}"#,
                commit.as_ref()
            )?;
            n_commits += 1;
        }
        query.push_str("}}");

        let request_body = serde_json::json!({
            "query": query
        });

        let graphql_response = loop {
            let mut request_builder = self
                .client
                .post(GITHUB_GRAPHQL_URL)
                .header(header::CONTENT_TYPE, "application/json")
                .header(header::USER_AGENT, &Self::user_agent());
            if let Some(token) = self.github_token() {
                request_builder = request_builder.bearer_auth(token);
            }
            let response = request_builder.json(&request_body).send().await?;

            if response.status().is_success() {
                break response.json::<GqlFetchSrcInfoResponse>().await?;
            } else {
                if let Some(retry_after) = response
                    .headers()
                    .get(header::RETRY_AFTER)
                    .and_then(|v| v.to_str().ok())
                {
                    // Handle standard Retry-After headers
                    let wait_time = if let Ok(retry_after_secs) = retry_after.parse::<u64>() {
                        retry_after_secs
                    } else if let Ok(date) = DateTime::parse_from_rfc2822(retry_after)
                        && date.timestamp() >= Utc::now().timestamp()
                    {
                        (date.timestamp() - Utc::now().timestamp()) as u64
                    } else {
                        warn!("Illegal Retry-After header: {retry_after}");
                        continue;
                    } + RETRY_AFTER_FINETUNING;
                    info!("Rate limited. Waiting {wait_time} seconds...");
                    sleep(Duration::from_secs(wait_time as u64)).await;

                    continue;
                } else if let Some("0") = response
                    .headers()
                    .get("x-ratelimit-remaining")
                    .and_then(|v| v.to_str().ok())
                {
                    // Handle GitHub-specific rate limit headers
                    let now = Utc::now().timestamp() as u64;
                    let rate_limit_reset = response
                        .headers()
                        .get("x-ratelimit-reset")
                        .and_then(|v| v.to_str().ok())
                        .and_then(|v| v.parse::<u64>().ok())
                        .unwrap_or(now);
                    let wait_time = rate_limit_reset - now + RETRY_AFTER_FINETUNING;

                    info!("Rate limited. Waiting {wait_time} seconds...");
                    sleep(Duration::from_secs(wait_time)).await;

                    continue;
                }
                Err(anyhow!("GitHub API error: {}", response.status()))?;
            }
        };

        if let Some(errors) = graphql_response.errors {
            Err(anyhow!("GraphQL errors: {:?}", errors))?;
        }

        let mut data = graphql_response
            .data
            .ok_or_else(|| anyhow!("No data in GraphQL response"))?;

        let result = (0..n_commits).map(move |i| {
            let key = format!("x{i}");
            data.repository
                .remove(&key)
                .map(|obj| obj.text)
                .unwrap_or_default()
        });

        Ok(result)
    }
}
