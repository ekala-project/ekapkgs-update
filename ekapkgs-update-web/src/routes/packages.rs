use axum::extract::{Path, Query, State};
use axum::response::IntoResponse;
use chrono::{DateTime, Utc};
use serde::Deserialize;
use sqlx;

use crate::state::AppState;
use crate::templates::{PackageInfo, PackagesTemplate};

#[derive(Deserialize)]
pub struct PackagesQuery {
    pub search: Option<String>,
}

pub async fn list(
    State(state): State<AppState>,
    Query(query): Query<PackagesQuery>,
) -> impl IntoResponse {
    // Query packages from updates table
    let sql = if let Some(search_term) = &query.search {
        format!(
            "SELECT attr_path, current_version, latest_upstream_version, last_attempted, \
             next_attempt, pr_url
             FROM updates
             WHERE attr_path LIKE '%{}%'
             ORDER BY last_attempted DESC NULLS LAST
             LIMIT 200",
            search_term.replace('\'', "''")
        )
    } else {
        "SELECT attr_path, current_version, latest_upstream_version, last_attempted, next_attempt, \
         pr_url
         FROM updates
         ORDER BY last_attempted DESC NULLS LAST
         LIMIT 200"
            .to_string()
    };

    let rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(&sql)
        .fetch_all(state.db.pool())
        .await
        .unwrap_or_default();

    let packages: Vec<PackageInfo> = rows
        .into_iter()
        .map(
            |(attr_path, current_version, latest_version, last_attempted, next_attempt, pr_url)| {
                let status =
                    determine_status(&current_version, &latest_version, &last_attempted, &pr_url);

                PackageInfo {
                    attr_path,
                    current_version,
                    latest_version,
                    status,
                    last_attempted: last_attempted.and_then(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .ok()
                            .map(|dt| dt.with_timezone(&Utc))
                    }),
                    next_attempt: next_attempt.and_then(|s| {
                        DateTime::parse_from_rfc3339(&s)
                            .ok()
                            .map(|dt| dt.with_timezone(&Utc))
                    }),
                    pr_url,
                }
            },
        )
        .collect();

    PackagesTemplate {
        packages,
        search: query.search,
    }
}

pub async fn detail(
    State(_state): State<AppState>,
    Path(_attr): Path<String>,
) -> impl IntoResponse {
    // TODO: Implement package detail view
    "Package detail view - Coming soon"
}

fn determine_status(
    current_version: &Option<String>,
    latest_version: &Option<String>,
    last_attempted: &Option<String>,
    pr_url: &Option<String>,
) -> String {
    if pr_url.is_some() {
        "pr_created".to_string()
    } else if last_attempted.is_some() {
        if current_version == latest_version {
            "up_to_date".to_string()
        } else {
            "update_failed".to_string()
        }
    } else {
        "pending".to_string()
    }
}
