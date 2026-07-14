//! Async data source for the inline repos menu.
//!
//! Historically this pulled the list of "previously opened git repos" from `PersistedWorkspace`.
//! After LSP + workspace history were retired, this candidate source no longer exists, so this data source
//! only keeps the trait and view wiring and always returns an empty result —— meaning the menu can still be
//! invoked but never has any candidates. This avoids a large rework of the upstream view / suggestions mode
//! wiring; if a "live cwd of the current pane group" source is wired in later, the data source can be restored.

#[cfg(feature = "local_fs")]
use std::collections::HashMap;
#[cfg(feature = "local_fs")]
use std::path::PathBuf;
#[cfg(feature = "local_fs")]
use std::sync::{Arc, Mutex};

use warpui::{AppContext, Entity};

use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::{AsyncDataSource, BoxFuture, DataSourceRunErrorWrapper};
use crate::terminal::input::repos::AcceptRepo;
#[cfg(feature = "local_fs")]
use crate::util::git::RepoGitSummary;

/// Cache of per-repo git summaries (branch + diff stats) keyed by repo path.
///
/// Shared between the data source, which reads it to render results immediately,
/// and the view, which populates it in the background. This lets the menu show
/// the repo list synchronously while the (relatively expensive) git data is
/// lazily loaded and filled in as it arrives.
#[cfg(feature = "local_fs")]
pub type GitSummaryCache = Arc<Mutex<HashMap<PathBuf, RepoGitSummary>>>;

pub struct RepoMenuDataSource {
    /// Git summaries populated in the background by the view. Reads never block
    /// on git; missing entries simply render without branch/diff-stat suffixes.
    #[cfg(feature = "local_fs")]
    git_summaries: GitSummaryCache,
}

impl RepoMenuDataSource {
    #[cfg(feature = "local_fs")]
    pub fn new(git_summaries: GitSummaryCache) -> Self {
        Self { git_summaries }
    }

    #[cfg(not(feature = "local_fs"))]
    pub fn new() -> Self {
        Self
    }
}

impl AsyncDataSource for RepoMenuDataSource {
    type Action = AcceptRepo;

    fn run_query(
        &self,
        _query: &Query,
        _app: &AppContext,
    ) -> BoxFuture<'static, Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper>> {
        Box::pin(async move { Ok(Vec::new()) })
    }
}

impl Entity for RepoMenuDataSource {
    type Event = ();
}
