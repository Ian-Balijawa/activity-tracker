use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header::AUTHORIZATION, HeaderMap},
    routing::get,
    Json, Router,
};
use chrono::{Datelike, NaiveDate, Utc};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::trace::TraceLayer;

use crate::config::Config;
use crate::error::AppError;
use crate::github::GithubApi;
use crate::models::ActivityReport;
use crate::service::{build_report, ReportParams};

const MAX_RANGE_DAYS: i64 = 366;

#[derive(Clone)]
pub struct AppState {
    pub http: reqwest::Client,
    pub config: Arc<Config>,
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/api/v1/orgs/{org}/activity", get(activity))
        .with_state(state)
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &axum::http::Request<_>| {
                    tracing::info_span!(
                        "http_request",
                        method = %request.method(),
                        uri = %request.uri(),
                        version = ?request.version(),
                    )
                })
                .on_request(tower_http::trace::DefaultOnRequest::new().level(tracing::Level::INFO))
                .on_response(
                    tower_http::trace::DefaultOnResponse::new()
                        .level(tracing::Level::INFO)
                        .latency_unit(tower_http::LatencyUnit::Millis),
                ),
        )
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

#[derive(Debug, Deserialize)]
pub struct ActivityQuery {
    /// Shortcut for a whole calendar month, e.g. "2026-09".
    month: Option<String>,
    from: Option<NaiveDate>,
    to: Option<NaiveDate>,
    /// GitHub login. Defaults to the owner of the token.
    author: Option<String>,
    #[serde(default)]
    include_commit_stats: bool,
    #[serde(default)]
    exclude_merges: bool,
}

async fn activity(
    State(state): State<AppState>,
    Path(org): Path<String>,
    Query(query): Query<ActivityQuery>,
    headers: HeaderMap,
) -> Result<Json<ActivityReport>, AppError> {
    validate_login("organization", &org)?;
    if let Some(author) = &query.author {
        validate_login("author", author)?;
    }
    let (from, to) = resolve_period(&query, Utc::now().date_naive())?;

    let token = resolve_token(&headers, &state.config)?;
    let api = GithubApi::new(
        state.http.clone(),
        state.config.github_api_url.clone(),
        token,
    );

    let report = build_report(
        &api,
        ReportParams {
            org,
            author: query.author,
            from,
            to,
            include_commit_stats: query.include_commit_stats,
            exclude_merges: query.exclude_merges,
        },
    )
    .await?;
    Ok(Json(report))
}

/// Token priority: `X-GitHub-Token` header, then `Authorization: Bearer`, then GITHUB_TOKEN.
fn resolve_token(headers: &HeaderMap, config: &Config) -> Result<String, AppError> {
    if let Some(token) = headers.get("x-github-token").and_then(|v| v.to_str().ok()) {
        return Ok(token.trim().to_string());
    }
    if let Some(token) = headers
        .get(AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
    {
        return Ok(token.trim().to_string());
    }
    config.github_token.clone().ok_or(AppError::MissingToken)
}

/// Logins and org names go into a search query, so allow only what GitHub allows.
fn validate_login(label: &str, value: &str) -> Result<(), AppError> {
    let is_valid = !value.is_empty()
        && value.len() <= 100
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.');
    if is_valid {
        Ok(())
    } else {
        Err(AppError::BadRequest(format!("invalid {label} '{value}'")))
    }
}

/// Turns the query into an inclusive date range. Defaults to this month so far.
fn resolve_period(
    query: &ActivityQuery,
    today: NaiveDate,
) -> Result<(NaiveDate, NaiveDate), AppError> {
    let (from, to) = match (&query.month, query.from, query.to) {
        (Some(_), Some(_), _) | (Some(_), _, Some(_)) => {
            return Err(AppError::BadRequest(
                "use either 'month' or 'from'/'to', not both".to_string(),
            ))
        }
        (Some(month), None, None) => {
            let first = NaiveDate::parse_from_str(&format!("{month}-01"), "%Y-%m-%d")
                .map_err(|_| AppError::BadRequest("month must look like 2026-09".to_string()))?;
            (first, last_day_of_month(first)?)
        }
        (None, Some(from), to) => (from, to.unwrap_or(today)),
        (None, None, Some(_)) => {
            return Err(AppError::BadRequest("'to' needs a 'from' date".to_string()))
        }
        (None, None, None) => (today.with_day(1).unwrap_or(today), today),
    };

    if from > to {
        return Err(AppError::BadRequest(
            "'from' must not be after 'to'".to_string(),
        ));
    }
    if (to - from).num_days() > MAX_RANGE_DAYS {
        return Err(AppError::BadRequest(format!(
            "date range is limited to {MAX_RANGE_DAYS} days"
        )));
    }
    Ok((from, to))
}

fn last_day_of_month(first_of_month: NaiveDate) -> Result<NaiveDate, AppError> {
    let (year, month) = (first_of_month.year(), first_of_month.month());
    let first_of_next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1)
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1)
    };
    first_of_next
        .and_then(|date| date.pred_opt())
        .ok_or_else(|| AppError::BadRequest("month is out of range".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(month: Option<&str>, from: Option<&str>, to: Option<&str>) -> ActivityQuery {
        let parse = |s: &str| NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap();
        ActivityQuery {
            month: month.map(str::to_string),
            from: from.map(parse),
            to: to.map(parse),
            author: None,
            include_commit_stats: false,
            exclude_merges: false,
        }
    }

    fn today() -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, 2).unwrap()
    }

    #[test]
    fn month_covers_whole_month() {
        let (from, to) = resolve_period(&query(Some("2026-02"), None, None), today()).unwrap();
        assert_eq!(from.to_string(), "2026-02-01");
        assert_eq!(to.to_string(), "2026-02-28");
    }

    #[test]
    fn december_rolls_over_the_year() {
        let (_, to) = resolve_period(&query(Some("2026-12"), None, None), today()).unwrap();
        assert_eq!(to.to_string(), "2026-12-31");
    }

    #[test]
    fn defaults_to_month_so_far() {
        let (from, to) = resolve_period(&query(None, None, None), today()).unwrap();
        assert_eq!(from.to_string(), "2026-10-01");
        assert_eq!(to, today());
    }

    #[test]
    fn rejects_mixed_and_reversed_ranges() {
        assert!(
            resolve_period(&query(Some("2026-09"), Some("2026-09-01"), None), today()).is_err()
        );
        assert!(resolve_period(
            &query(None, Some("2026-09-30"), Some("2026-09-01")),
            today()
        )
        .is_err());
    }

    #[test]
    fn rejects_unsafe_logins() {
        assert!(validate_login("author", "ian-b").is_ok());
        assert!(validate_login("author", "ian author:other").is_err());
    }
}
