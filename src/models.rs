//! The JSON shape this API returns. Designed so a report template can map onto it directly.

use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct ActivityReport {
    pub organization: String,
    pub author: String,
    pub period: Period,
    pub generated_at: DateTime<Utc>,
    pub summary: Summary,
    pub repositories: Vec<RepositoryActivity>,
}

#[derive(Debug, Serialize)]
pub struct Period {
    pub from: NaiveDate,
    pub to: NaiveDate,
}

#[derive(Debug, Default, Serialize)]
pub struct Summary {
    pub repositories: usize,
    pub commits: usize,
    pub pull_requests: usize,
    pub pull_requests_merged: usize,
    pub pull_requests_open: usize,
    pub pull_requests_closed_unmerged: usize,
    pub pr_lines_added: u64,
    pub pr_lines_removed: u64,
    /// Only present when `include_commit_stats=true`.
    pub commit_lines_added: Option<u64>,
    pub commit_lines_removed: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct RepositoryActivity {
    pub name: String,
    pub full_name: String,
    pub description: Option<String>,
    pub url: String,
    pub private: bool,
    pub language: Option<String>,
    pub commit_count: usize,
    pub pull_request_count: usize,
    pub first_activity: Option<DateTime<Utc>>,
    pub last_activity: Option<DateTime<Utc>>,
    pub commits: Vec<CommitActivity>,
    pub pull_requests: Vec<PullRequestActivity>,
}

#[derive(Debug, Serialize)]
pub struct CommitActivity {
    pub sha: String,
    pub short_sha: String,
    pub title: String,
    pub message: String,
    pub committed_at: DateTime<Utc>,
    pub url: String,
    pub is_merge: bool,
    pub additions: Option<u64>,
    pub deletions: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct PullRequestActivity {
    pub number: u64,
    pub title: String,
    pub description: Option<String>,
    /// One of "merged", "open", "closed".
    pub state: String,
    pub draft: bool,
    pub url: String,
    pub created_at: DateTime<Utc>,
    pub merged_at: Option<DateTime<Utc>>,
    pub closed_at: Option<DateTime<Utc>>,
    pub base_branch: String,
    pub head_branch: String,
    pub labels: Vec<String>,
    pub commits: u64,
    pub changed_files: u64,
    pub additions: u64,
    pub deletions: u64,
}
