//! Collects commits and pull requests for one author across an organization.

use std::collections::{BTreeSet, HashMap, HashSet};

use chrono::{DateTime, NaiveDate, NaiveTime, Utc};
use futures::{stream, StreamExt, TryStreamExt};

use crate::error::AppError;
use crate::github::{GhCommit, GhPullRequest, GhRepo, GithubApi, PullRequestRef};
use crate::models::{
    ActivityReport, CommitActivity, Meta, Period, PullRequestActivity, RepositoryActivity, Summary,
};

/// Maximum number of GitHub requests in flight at once.
const CONCURRENCY: usize = 8;

#[derive(Debug)]
pub struct ReportParams {
    pub org: String,
    pub author: Option<String>,
    pub from: NaiveDate,
    pub to: NaiveDate,
    pub include_commit_stats: bool,
    pub exclude_merges: bool,
}

pub async fn build_report(
    api: &GithubApi,
    params: ReportParams,
) -> Result<ActivityReport, AppError> {
    let author = match &params.author {
        Some(author) => author.clone(),
        None => api.current_user().await?.login,
    };
    let since = start_of_day(params.from);
    let until = end_of_day(params.to);

    // Repo discovery and PR search are independent, so run them together.
    let (repos, pull_request_refs) = tokio::try_join!(
        api.list_org_repos(&params.org, since),
        find_pull_requests(api, &params.org, &author, params.from, params.to),
    )?;

    let repositories_scanned = repos.len();
    let pull_requests_found = pull_request_refs.len();
    tracing::info!(
        org = %params.org, %author, repositories_scanned, pull_requests_found,
        "discovery finished"
    );

    // Commits per repository.
    let author_ref = &author;
    let commit_results: Vec<(GhRepo, Vec<GhCommit>)> = stream::iter(repos)
        .map(|repo| async move {
            let commits = api
                .list_commits(&repo.full_name, author_ref, since, until)
                .await?;
            Ok::<_, AppError>((repo, commits))
        })
        .buffer_unordered(CONCURRENCY)
        .try_collect()
        .await?;

    let mut repo_map: HashMap<String, GhRepo> = HashMap::new();
    let mut repo_commits: HashMap<String, Vec<GhCommit>> = HashMap::new();
    for (repo, mut commits) in commit_results {
        if params.exclude_merges {
            commits.retain(|commit| commit.parents.len() <= 1);
        }
        let key = repo.full_name.to_lowercase();
        if !commits.is_empty() {
            repo_commits.insert(key.clone(), commits);
        }
        repo_map.insert(key, repo);
    }

    // Repositories that only show up through pull requests still need their details.
    let missing: Vec<String> = pull_request_refs
        .iter()
        .filter(|pr| !repo_map.contains_key(&pr.repo_full_name.to_lowercase()))
        .map(|pr| pr.repo_full_name.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let extra_repos: Vec<GhRepo> = stream::iter(missing)
        .map(|full_name| async move { api.get_repo(&full_name).await })
        .buffer_unordered(CONCURRENCY)
        .try_collect()
        .await?;
    for repo in extra_repos {
        repo_map.insert(repo.full_name.to_lowercase(), repo);
    }

    // Pull request details (line counts, merge state, branches).
    let pull_requests: Vec<(String, GhPullRequest)> = stream::iter(pull_request_refs)
        .map(|pr| async move {
            let detail = api.get_pull_request(&pr.repo_full_name, pr.number).await?;
            Ok::<_, AppError>((pr.repo_full_name.to_lowercase(), detail))
        })
        .buffer_unordered(CONCURRENCY)
        .try_collect()
        .await?;
    let mut repo_prs: HashMap<String, Vec<PullRequestActivity>> = HashMap::new();
    for (key, pr) in pull_requests {
        repo_prs.entry(key).or_default().push(to_pull_request(pr));
    }

    // Optional per-commit line statistics.
    let commit_stats = if params.include_commit_stats {
        let targets: Vec<(String, String)> = repo_commits
            .iter()
            .flat_map(|(key, commits)| {
                let full_name = repo_map[key].full_name.clone();
                commits
                    .iter()
                    .map(move |commit| (full_name.clone(), commit.sha.clone()))
            })
            .collect();
        let details: Vec<GhCommit> = stream::iter(targets)
            .map(|(full_name, sha)| async move { api.get_commit(&full_name, &sha).await })
            .buffer_unordered(CONCURRENCY)
            .try_collect()
            .await?;
        Some(
            details
                .into_iter()
                .filter_map(|commit| commit.stats.map(|stats| (commit.sha, stats)))
                .collect::<HashMap<_, _>>(),
        )
    } else {
        None
    };

    // Assemble, keeping only repositories with activity.
    let mut repositories = Vec::new();
    for (key, repo) in &repo_map {
        let mut commits: Vec<CommitActivity> = repo_commits
            .remove(key)
            .unwrap_or_default()
            .into_iter()
            .map(|commit| to_commit(commit, commit_stats.as_ref()))
            .collect();
        let mut prs = repo_prs.remove(key).unwrap_or_default();
        if commits.is_empty() && prs.is_empty() {
            continue;
        }
        commits.sort_by_key(|commit| commit.committed_at);
        prs.sort_by_key(|pr| pr.created_at);

        let timestamps: Vec<DateTime<Utc>> = commits
            .iter()
            .map(|commit| commit.committed_at)
            .chain(prs.iter().map(|pr| pr.created_at))
            .collect();

        repositories.push(RepositoryActivity {
            name: repo.name.clone(),
            full_name: repo.full_name.clone(),
            description: repo.description.clone(),
            url: repo.html_url.clone(),
            private: repo.private,
            language: repo.language.clone(),
            commit_count: commits.len(),
            pull_request_count: prs.len(),
            first_activity: timestamps.iter().min().copied(),
            last_activity: timestamps.iter().max().copied(),
            commits,
            pull_requests: prs,
        });
    }
    repositories.sort_by(|a, b| a.full_name.to_lowercase().cmp(&b.full_name.to_lowercase()));

    let summary = summarize(&repositories, params.include_commit_stats);
    let warnings = build_warnings(
        &params.org,
        &author,
        repositories_scanned,
        pull_requests_found,
        &summary,
    );

    Ok(ActivityReport {
        organization: params.org,
        author,
        period: Period {
            from: params.from,
            to: params.to,
        },
        generated_at: Utc::now(),
        summary,
        meta: Meta {
            repositories_scanned,
            pull_requests_found,
            warnings,
        },
        repositories,
    })
}

fn build_warnings(
    org: &str,
    author: &str,
    repositories_scanned: usize,
    pull_requests_found: usize,
    summary: &Summary,
) -> Vec<String> {
    let mut warnings = Vec::new();
    if repositories_scanned == 0 && pull_requests_found == 0 {
        warnings.push(format!(
            "GitHub showed no repositories or pull requests in '{org}' for this token. \
             A fine-grained token must list '{org}' as its resource owner and be approved by the org. \
             Also check the org spelling and, for SAML orgs, authorize the token for SSO."
        ));
    } else if summary.commits == 0 && summary.pull_requests == 0 {
        warnings.push(format!(
            "Repositories were scanned but nothing matched author '{author}' in this period. \
             Commits only match when the commit email is linked to that GitHub account."
        ));
    }
    warnings
}

/// Finds PRs by the author that were created or merged inside the window.
async fn find_pull_requests(
    api: &GithubApi,
    org: &str,
    author: &str,
    from: NaiveDate,
    to: NaiveDate,
) -> Result<Vec<PullRequestRef>, AppError> {
    let created = format!("org:{org} is:pr author:{author} created:{from}..{to}");
    let merged = format!("org:{org} is:pr author:{author} merged:{from}..{to}");

    let (created, merged) = tokio::try_join!(
        api.search_pull_requests(&created),
        api.search_pull_requests(&merged),
    )?;

    let mut seen = HashSet::new();
    Ok(created
        .into_iter()
        .chain(merged)
        .filter(|pr| seen.insert(pr.clone()))
        .collect())
}

fn to_commit(
    commit: GhCommit,
    stats: Option<&HashMap<String, crate::github::GhCommitStats>>,
) -> CommitActivity {
    let title = commit.commit.message.lines().next().unwrap_or("").to_string();
    let line_stats = stats.and_then(|map| map.get(&commit.sha));
    CommitActivity {
        short_sha: commit.sha.chars().take(7).collect(),
        title,
        message: commit.commit.message.trim().to_string(),
        committed_at: commit.commit.author.date,
        url: commit.html_url,
        is_merge: commit.parents.len() > 1,
        additions: line_stats.map(|s| s.additions),
        deletions: line_stats.map(|s| s.deletions),
        sha: commit.sha,
    }
}

fn to_pull_request(pr: GhPullRequest) -> PullRequestActivity {
    let state = if pr.merged_at.is_some() {
        "merged".to_string()
    } else {
        pr.state
    };
    PullRequestActivity {
        number: pr.number,
        title: pr.title,
        description: pr.body.filter(|body| !body.trim().is_empty()),
        state,
        draft: pr.draft,
        url: pr.html_url,
        created_at: pr.created_at,
        merged_at: pr.merged_at,
        closed_at: pr.closed_at,
        base_branch: pr.base.name,
        head_branch: pr.head.name,
        labels: pr.labels.into_iter().map(|label| label.name).collect(),
        commits: pr.commits,
        changed_files: pr.changed_files,
        additions: pr.additions,
        deletions: pr.deletions,
    }
}

fn summarize(repositories: &[RepositoryActivity], with_commit_stats: bool) -> Summary {
    let mut summary = Summary {
        repositories: repositories.len(),
        commit_lines_added: with_commit_stats.then_some(0),
        commit_lines_removed: with_commit_stats.then_some(0),
        ..Summary::default()
    };

    for repo in repositories {
        summary.commits += repo.commits.len();
        for commit in &repo.commits {
            if let Some(total) = summary.commit_lines_added.as_mut() {
                *total += commit.additions.unwrap_or(0);
            }
            if let Some(total) = summary.commit_lines_removed.as_mut() {
                *total += commit.deletions.unwrap_or(0);
            }
        }
        for pr in &repo.pull_requests {
            summary.pull_requests += 1;
            summary.pr_lines_added += pr.additions;
            summary.pr_lines_removed += pr.deletions;
            match pr.state.as_str() {
                "merged" => summary.pull_requests_merged += 1,
                "open" => summary.pull_requests_open += 1,
                _ => summary.pull_requests_closed_unmerged += 1,
            }
        }
    }
    summary
}

fn start_of_day(date: NaiveDate) -> DateTime<Utc> {
    date.and_time(NaiveTime::MIN).and_utc()
}

fn end_of_day(date: NaiveDate) -> DateTime<Utc> {
    date.and_hms_opt(23, 59, 59)
        .expect("23:59:59 is a valid time")
        .and_utc()
}
