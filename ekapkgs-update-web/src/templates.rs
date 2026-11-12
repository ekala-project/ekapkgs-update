//! Askama templates. Each page has a struct; dashboard panels and live
//! regions also have fragment structs that share the page's field names, so
//! the page can `{% include %}` the fragment for the initial render and HTMX
//! can fetch the same fragment later.

use askama_axum::Template;

use crate::routes::analytics::AnalyticsView;
use crate::routes::dashboard::{ActiveView, ActivityView, StatsView, WatchPanelView};
use crate::routes::failures::FailureListView;
use crate::routes::packages::{PackageDetailView, PackageListView};
use crate::routes::sessions::{SessionDetailView, SessionListView};
use crate::routes::watch::WatchPageView;

#[derive(Template)]
#[template(path = "error.html")]
pub struct ErrorTemplate {
    pub code: u16,
    pub title: String,
    pub message: String,
}

#[derive(Template)]
#[template(path = "dashboard.html")]
pub struct DashboardTemplate {
    pub stats: StatsView,
    pub active: ActiveView,
    pub watch: WatchPanelView,
    pub activity: ActivityView,
}

#[derive(Template)]
#[template(path = "fragments/stats.html")]
pub struct StatsFragment {
    pub stats: StatsView,
}

#[derive(Template)]
#[template(path = "fragments/active.html")]
pub struct ActiveFragment {
    pub active: ActiveView,
}

#[derive(Template)]
#[template(path = "fragments/watch_panel.html")]
pub struct WatchPanelFragment {
    pub watch: WatchPanelView,
}

#[derive(Template)]
#[template(path = "fragments/activity.html")]
pub struct ActivityFragment {
    pub activity: ActivityView,
}

#[derive(Template)]
#[template(path = "packages.html")]
pub struct PackagesTemplate {
    pub list: PackageListView,
}

#[derive(Template)]
#[template(path = "package_detail.html")]
pub struct PackageDetailTemplate {
    pub d: PackageDetailView,
}

#[derive(Template)]
#[template(path = "sessions.html")]
pub struct SessionsTemplate {
    pub list: SessionListView,
}

#[derive(Template)]
#[template(path = "fragments/sessions_table.html")]
pub struct SessionsFragment {
    pub list: SessionListView,
}

#[derive(Template)]
#[template(path = "session_detail.html")]
pub struct SessionDetailTemplate {
    pub s: SessionDetailView,
}

#[derive(Template)]
#[template(path = "fragments/session_live.html")]
pub struct SessionLiveFragment {
    pub s: SessionDetailView,
}

#[derive(Template)]
#[template(path = "failures.html")]
pub struct FailuresTemplate {
    pub list: FailureListView,
}

#[derive(Template)]
#[template(path = "watch.html")]
pub struct WatchTemplate {
    pub w: WatchPageView,
}

#[derive(Template)]
#[template(path = "analytics.html")]
pub struct AnalyticsTemplate {
    pub a: AnalyticsView,
}
