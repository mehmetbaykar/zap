//! Async data source for the inline repos menu.
//!
//! Historically this pulled the list of "previously opened git repos" from `PersistedWorkspace`.
//! After LSP + workspace history were retired, this candidate source no longer exists, so this data source
//! only keeps the trait and view wiring and always returns an empty result —— meaning the menu can still be
//! invoked but never has any candidates. This avoids a large rework of the upstream view / suggestions mode
//! wiring; if a "live cwd of the current pane group" source is wired in later, the data source can be restored.

use warpui::{AppContext, Entity};

use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::{AsyncDataSource, BoxFuture, DataSourceRunErrorWrapper};
use crate::terminal::input::repos::AcceptRepo;

pub struct RepoMenuDataSource;

impl RepoMenuDataSource {
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
