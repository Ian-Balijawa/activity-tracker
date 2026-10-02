//! Thin async client for the parts of the GitHub REST API this tool needs.

use std::time::Duration;

use chrono::{DateTime, Utc};
use reqwest::{
    header::{HeaderMap, HeaderValue, ACCEPT, LINK},
    Client, Response, StatusCode,
};
use serde::{de::DeserializeOwned, Deserialize};

use crate::error::AppError;

const MAX_RETRIES: u32 = 3;
const MAX_PAGES: u32 = 50;
const MAX_SEARCH_PAGES: u32 = 10;

// ---------------------------------------------------------------------------
// GitHub response models (only the fields we use)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct GhUser {
    pub login: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhRepo {
    pub name: String,
    pub full_name: String,
    pub description: Option<String>,
    pub html_url: String,
    pub private: bool,
    pub language: Option<String>,
    pub pushed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Deserialize)]
pub struct GhCommit {
    pub sha: String,
    pub html_url: String,
    pub commit: GhCommitData,
    #[serde(default)]
    pub parents: Vec<serde_json::Value>,
    pub stats: Option<GhCommitStats>,
}

#[derive(Debug, Deserialize)]
pub struct GhCommitData {
    pub message: String,
    pub author: GhGitActor,
}

#[derive(Debug, Deserialize)]
pub struct GhGitActor {
    pub date: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
pub struct GhCommitStats {
    pub additions: u64,
    pub deletions: u64,
}

#[derive(Debug, Deserialize)]
struct GhSearchResponse {
    items: Vec<GhSearchItem>,
}

#[derive(Debug, Deserialize)]
struct GhSearchItem {
    number: u64,
    repository_url: String,
}

/// A pull request found by search: the repository full name and the PR number.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PullRequestRef {
    pub repo_full_name: String,
    pub number: u64,
}

#[derive(Debug, Deserialize)]
pub struct GhPullRequest {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub draft: bool,
    pub html_url: String,
    pub body: Option<String>,
    pub created_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
    pub merged_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    #[serde(default)]
    pub changed_files: u64,
    #[serde(default)]
    pub commits: u64,
    #[serde(default)]
    pub labels: Vec<GhLabel>,
    pub base: GhBranch,
    pub head: GhBranch,
}

#[derive(Debug, Deserialize)]
pub struct GhLabel {
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct GhBranch {
    #[serde(rename = "ref")]
    pub name: String,
}

#[derive(Deserialize)]
struct GhErrorBody {
    message: String,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// Builds the shared HTTP client with the headers GitHub expects.
pub fn build_http_client() -> Result<Client, reqwest::Error> {
    let mut headers = HeaderMap::new();
    headers.insert(ACCEPT, HeaderValue::from_static("application/vnd.github+json"));
    headers.insert(
        "x-github-api-version",
        HeaderValue::from_static("2022-11-28"),
    );
    Client::builder()
        .user_agent("activity-tracker/0.1")
        .default_headers(headers)
        .timeout(Duration::from_secs(30))
        .build()
}

/// A GitHub API handle bound to one token. Cheap to create per request.
#[derive(Clone)]
pub struct GithubApi {
    http: Client,
    base_url: String,
    token: String,
}

impl GithubApi {
    pub fn new(http: Client, base_url: String, token: String) -> Self {
        Self {
            http,
            base_url,
            token,
        }
    }

    pub async fn current_user(&self) -> Result<GhUser, AppError> {
        let url = format!("{}/user", self.base_url);
        Ok(self.get(&url, &[]).await?.json().await?)
    }

    pub async fn get_repo(&self, full_name: &str) -> Result<GhRepo, AppError> {
        let url = format!("{}/repos/{}", self.base_url, full_name);
        Ok(self.get(&url, &[]).await?.json().await?)
    }

    /// Lists org repositories pushed to on or after `since`, newest push first.
    /// Stops paging as soon as it reaches older repositories.
    pub async fn list_org_repos(
        &self,
        org: &str,
        since: DateTime<Utc>,
    ) -> Result<Vec<GhRepo>, AppError> {
        let url = format!("{}/orgs/{}/repos", self.base_url, org);
        let mut repos = Vec::new();

        for page in 1..=MAX_PAGES {
            let query = [
                ("type", "all".to_string()),
                ("sort", "pushed".to_string()),
                ("direction", "desc".to_string()),
                ("per_page", "100".to_string()),
                ("page", page.to_string()),
            ];
            let batch: Vec<GhRepo> = self.get(&url, &query).await?.json().await?;
            let batch_len = batch.len();
            let reached_older = batch
                .iter()
                .any(|repo| repo.pushed_at.map_or(true, |pushed| pushed < since));

            repos.extend(
                batch
                    .into_iter()
                    .filter(|repo| repo.pushed_at.is_some_and(|pushed| pushed >= since)),
            );

            if batch_len < 100 || reached_older {
                break;
            }
        }
        Ok(repos)
    }

    /// Lists default-branch commits by `author` in the window. An empty repo yields no commits.
    pub async fn list_commits(
        &self,
        repo_full_name: &str,
        author: &str,
        since: DateTime<Utc>,
        until: DateTime<Utc>,
    ) -> Result<Vec<GhCommit>, AppError> {
        let path = format!("/repos/{repo_full_name}/commits");
        let query = [
            ("author", author.to_string()),
            ("since", since.to_rfc3339()),
            ("until", until.to_rfc3339()),
            ("per_page", "100".to_string()),
        ];
        match self.get_all(&path, &query).await {
            // GitHub answers 409 for a repository with no commits.
            Err(AppError::Upstream { status: 409, .. }) => Ok(Vec::new()),
            other => other,
        }
    }

    /// Fetches one commit with its line statistics.
    pub async fn get_commit(&self, repo_full_name: &str, sha: &str) -> Result<GhCommit, AppError> {
        let url = format!("{}/repos/{}/commits/{}", self.base_url, repo_full_name, sha);
        Ok(self.get(&url, &[]).await?.json().await?)
    }

    pub async fn get_pull_request(
        &self,
        repo_full_name: &str,
        number: u64,
    ) -> Result<GhPullRequest, AppError> {
        let url = format!("{}/repos/{}/pulls/{}", self.base_url, repo_full_name, number);
        Ok(self.get(&url, &[]).await?.json().await?)
    }

    /// Runs an issue search and returns the pull requests it matched.
    pub async fn search_pull_requests(&self, query: &str) -> Result<Vec<PullRequestRef>, AppError> {
        let url = format!("{}/search/issues", self.base_url);
        let mut found = Vec::new();

        for page in 1..=MAX_SEARCH_PAGES {
            let params = [
                ("q", query.to_string()),
                ("per_page", "100".to_string()),
                ("page", page.to_string()),
            ];
            let response: GhSearchResponse = self.get(&url, &params).await?.json().await?;
            let batch_len = response.items.len();

            found.extend(response.items.into_iter().filter_map(|item| {
                repo_full_name_from_url(&item.repository_url).map(|repo_full_name| PullRequestRef {
                    repo_full_name,
                    number: item.number,
                })
            }));

            if batch_len < 100 {
                break;
            }
        }
        Ok(found)
    }

    // -- plumbing -----------------------------------------------------------

    /// Follows `Link: rel="next"` headers and gathers every page.
    async fn get_all<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, String)],
    ) -> Result<Vec<T>, AppError> {
        let mut items = Vec::new();
        let mut url = format!("{}{}", self.base_url, path);
        let mut query = query.to_vec();

        for _ in 0..MAX_PAGES {
            let response = self.get(&url, &query).await?;
            let next = next_link(response.headers());
            let page: Vec<T> = response.json().await?;
            items.extend(page);

            match next {
                Some(next_url) => {
                    url = next_url;
                    query.clear(); // the next link already carries the query string
                }
                None => break,
            }
        }
        Ok(items)
    }

    /// Sends a GET with auth, retrying transient failures with backoff.
    async fn get(&self, url: &str, query: &[(&str, String)]) -> Result<Response, AppError> {
        let mut attempt = 0;
        loop {
            let sent = self
                .http
                .get(url)
                .query(query)
                .bearer_auth(&self.token)
                .send()
                .await;

            let response = match sent {
                Ok(response) => response,
                Err(error)
                    if attempt < MAX_RETRIES && (error.is_timeout() || error.is_connect()) =>
                {
                    backoff(attempt).await;
                    attempt += 1;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };

            let status = response.status();
            if status.is_success() {
                return Ok(response);
            }
            if status.is_server_error() && attempt < MAX_RETRIES {
                backoff(attempt).await;
                attempt += 1;
                continue;
            }
            return Err(error_from_response(response).await);
        }
    }
}

async fn backoff(attempt: u32) {
    tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
}

async fn error_from_response(response: Response) -> AppError {
    let status = response.status();
    let remaining = header_i64(response.headers(), "x-ratelimit-remaining");
    let reset = header_i64(response.headers(), "x-ratelimit-reset");
    let has_retry_after = response.headers().contains_key("retry-after");
    let body = response.text().await.unwrap_or_default();
    let message = serde_json::from_str::<GhErrorBody>(&body)
        .map(|parsed| parsed.message)
        .unwrap_or(body);

    let is_rate_limited = status == StatusCode::TOO_MANY_REQUESTS
        || remaining == Some(0)
        || has_retry_after
        || message.to_lowercase().contains("rate limit");

    match status {
        StatusCode::UNAUTHORIZED => AppError::Unauthorized,
        StatusCode::FORBIDDEN | StatusCode::TOO_MANY_REQUESTS if is_rate_limited => {
            AppError::RateLimited { reset }
        }
        StatusCode::FORBIDDEN => AppError::Forbidden(message),
        StatusCode::NOT_FOUND => AppError::NotFound(message),
        other => AppError::Upstream {
            status: other.as_u16(),
            message,
        },
    }
}

fn header_i64(headers: &HeaderMap, name: &str) -> Option<i64> {
    headers.get(name)?.to_str().ok()?.parse().ok()
}

/// Extracts the `rel="next"` URL from a GitHub `Link` header.
fn next_link(headers: &HeaderMap) -> Option<String> {
    let link = headers.get(LINK)?.to_str().ok()?;
    link.split(',').find_map(|part| {
        let (url, rel) = part.split_once(';')?;
        rel.contains("rel=\"next\"").then(|| {
            url.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .to_string()
        })
    })
}

/// "https://api.github.com/repos/acme/web" becomes "acme/web".
fn repo_full_name_from_url(repository_url: &str) -> Option<String> {
    let mut segments = repository_url.trim_end_matches('/').rsplit('/');
    let repo = segments.next()?;
    let owner = segments.next()?;
    Some(format!("{owner}/{repo}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_next_link() {
        let mut headers = HeaderMap::new();
        headers.insert(
            LINK,
            HeaderValue::from_static(
                "<https://api.github.com/x?page=2>; rel=\"next\", <https://api.github.com/x?page=9>; rel=\"last\"",
            ),
        );
        assert_eq!(
            next_link(&headers).as_deref(),
            Some("https://api.github.com/x?page=2")
        );
    }

    #[test]
    fn no_next_link_on_last_page() {
        let mut headers = HeaderMap::new();
        headers.insert(
            LINK,
            HeaderValue::from_static("<https://api.github.com/x?page=1>; rel=\"prev\""),
        );
        assert_eq!(next_link(&headers), None);
    }

    #[test]
    fn extracts_repo_full_name() {
        assert_eq!(
            repo_full_name_from_url("https://api.github.com/repos/acme/web").as_deref(),
            Some("acme/web")
        );
    }
}
