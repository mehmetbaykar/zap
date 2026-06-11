use fuzzy_match::{match_indices_case_insensitive, FuzzyMatchResult};
use itertools::Itertools;
use warpui::{AppContext, Entity};

use super::SshServerSearchItem;
use crate::search::command_palette::mixer::CommandPaletteItemAction;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::{DataSourceRunErrorWrapper, SyncDataSource};

use warp_ssh_manager::{NodeKind, SshRepository};

/// Upper bound. SSH typically has a few to a few dozen, so this won't blow up.
const MAX_SSH_SERVERS_CONSIDERED: usize = 200;

#[derive(Default)]
pub struct SshServersDataSource;

impl SshServersDataSource {
    pub fn new() -> Self {
        Self
    }
}

impl Entity for SshServersDataSource {
    type Event = ();
}

impl SyncDataSource for SshServersDataSource {
    type Action = CommandPaletteItemAction;

    fn run_query(
        &self,
        query: &Query,
        _app: &AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        // Use our own with_conn (an independent write connection) so we don't pollute PaneGroup's main write thread.
        // DataSourceRunErrorWrapper is a Box<dyn DataSourceRunError> custom trait,
        // and wrapping it is too costly — on failure, log + return an empty result (SSH is not shown in the
        // palette, but other sources are unaffected).
        let nodes = match warp_ssh_manager::with_conn(|c| Ok(SshRepository::list_nodes(c)?)) {
            Ok(n) => n,
            Err(e) => {
                log::warn!("command palette ssh: failed to load nodes: {e}");
                return Ok(Vec::new());
            }
        };

        // Only show server nodes. Fetch details for each node once, skipping failures (folders have no details and become None).
        let server_nodes: Vec<_> = nodes
            .into_iter()
            .filter(|n| matches!(n.kind, NodeKind::Server))
            .take(MAX_SSH_SERVERS_CONSIDERED)
            .collect();

        let query_str = query.text.as_str();
        let results = server_nodes
            .into_iter()
            .filter_map(|node| {
                let server =
                    warp_ssh_manager::with_conn(|c| Ok(SshRepository::get_server(c, &node.id)?))
                        .ok()
                        .flatten()?;

                // Use name + " " + host as the search text; a match on either name or host works.
                let display_name = node.name.clone();
                let host_user = if server.username.is_empty() {
                    server.host.clone()
                } else {
                    format!("{}@{}", server.username, server.host)
                };
                let haystack = format!("{display_name} {host_user}");

                let match_result = if query_str.is_empty() {
                    Some(FuzzyMatchResult::no_match())
                } else {
                    match_indices_case_insensitive(&haystack, query_str)
                }?;

                let mut item = SshServerSearchItem::new(node, server, host_user, display_name);
                let mut mr = match_result;
                // Boost slightly like RepoDataSource so ssh results stay competitive in the mixed panel.
                mr.score *= 4;
                item.match_result = mr;
                Some(item.into())
            })
            .collect_vec();

        Ok(results)
    }
}
