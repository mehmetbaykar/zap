use ai::skills::SkillPathOrigin;
use fuzzy_match::FuzzyMatchResult;
use warp_util::local_or_remote_path::LocalOrRemotePath;
use warpui::{AppContext, Entity, SingletonEntity};

use super::search_item::SkillSearchItem;
#[cfg(not(target_family = "wasm"))]
use crate::ai::blocklist::SessionContext;
use crate::ai::skills::SkillManager;
use crate::search::ai_context_menu::mixer::AIContextMenuSearchableAction;
use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::{DataSourceRunErrorWrapper, SyncDataSource};
#[cfg(not(target_family = "wasm"))]
use crate::workspace::ActiveSession;

const MAX_RESULTS: usize = 50;

pub struct SkillsDataSource;

impl SkillsDataSource {
    pub fn new() -> Self {
        Self
    }
}

impl SyncDataSource for SkillsDataSource {
    type Action = AIContextMenuSearchableAction;

    fn run_query(
        &self,
        query: &Query,
        app: &AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        let query_text = &query.text;

        // Resolve the current working directory and execution host from the active window's
        // session: an SSH session without a connected SSH extension has no cwd location and must
        // not fall back to this machine's skills.
        #[cfg(not(target_family = "wasm"))]
        let (cwd, path_origin) = app
            .windows()
            .state()
            .active_window
            .map(|window_id| {
                let active_session = ActiveSession::as_ref(app);
                let session_type = active_session
                    .session(window_id)
                    .map(|session| session.session_type());
                (
                    active_session.working_directory(window_id).cloned(),
                    SessionContext::skill_path_origin_for_session_type(session_type.as_ref()),
                )
            })
            .unwrap_or((None, SkillPathOrigin::Local));
        #[cfg(target_family = "wasm")]
        let (cwd, path_origin): (Option<LocalOrRemotePath>, SkillPathOrigin) =
            (None, SkillPathOrigin::Local);
        let skills = SkillManager::as_ref(app).get_skills_for_working_directory_with_origin(
            cwd.as_ref(),
            &path_origin,
            app,
        );

        let mut results: Vec<QueryResult<Self::Action>> = if query_text.is_empty() {
            // Zero state: show all skills with a uniform high score.
            skills
                .into_iter()
                .map(|skill| {
                    QueryResult::from(SkillSearchItem {
                        name: skill.name,
                        description: skill.description,
                        provider: skill.provider,
                        icon_override: skill.icon_override,
                        match_result: FuzzyMatchResult {
                            score: 1000,
                            matched_indices: vec![],
                        },
                    })
                })
                .collect()
        } else {
            // Fuzzy match against skill name.
            skills
                .into_iter()
                .filter_map(|skill| {
                    let match_result =
                        fuzzy_match::match_indices_case_insensitive(&skill.name, query_text)?;
                    // Skip very weak matches once the user has typed more than one character.
                    if query_text.len() > 1 && match_result.score < 10 {
                        return None;
                    }
                    Some(QueryResult::from(SkillSearchItem {
                        name: skill.name,
                        description: skill.description,
                        provider: skill.provider,
                        icon_override: skill.icon_override,
                        match_result,
                    }))
                })
                .collect()
        };

        results.sort_by_key(|r| std::cmp::Reverse(r.score()));
        results.truncate(MAX_RESULTS);

        Ok(results)
    }
}

impl Entity for SkillsDataSource {
    type Event = ();
}
