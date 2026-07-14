use warpui::{Entity, ModelHandle};

use crate::search::data_source::{Query, QueryResult};
use crate::search::mixer::DataSourceRunErrorWrapper;
use crate::search::SyncDataSource;
use crate::terminal::input::slash_commands::{
    AcceptSlashCommandOrSavedPrompt, GuiSlashCommandDataSource, SlashCommandDataSource,
    TuiSlashCommandDataSource,
};

pub struct GuiZeroStateDataSource {
    slash_command_data_source: ModelHandle<GuiSlashCommandDataSource>,
}

impl GuiZeroStateDataSource {
    pub fn new(slash_command_data_source: &ModelHandle<GuiSlashCommandDataSource>) -> Self {
        Self {
            slash_command_data_source: slash_command_data_source.clone(),
        }
    }
}

impl Entity for GuiZeroStateDataSource {
    type Event = ();
}

impl SyncDataSource for GuiZeroStateDataSource {
    type Action = AcceptSlashCommandOrSavedPrompt;

    fn run_query(
        &self,
        query: &Query,
        app: &warpui::AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        if !query.text.is_empty() {
            return Ok(vec![]);
        }

        Ok(self
            .slash_command_data_source
            .as_ref(app)
            .ordered_zero_state_commands(app)
            .into_iter()
            .map(Into::into)
            .collect())
    }
}

pub struct TuiZeroStateDataSource {
    slash_command_data_source: ModelHandle<TuiSlashCommandDataSource>,
}

impl TuiZeroStateDataSource {
    pub fn new(slash_command_data_source: &ModelHandle<TuiSlashCommandDataSource>) -> Self {
        Self {
            slash_command_data_source: slash_command_data_source.clone(),
        }
    }
}

impl Entity for TuiZeroStateDataSource {
    type Event = ();
}

impl SyncDataSource for TuiZeroStateDataSource {
    type Action = AcceptSlashCommandOrSavedPrompt;

    fn run_query(
        &self,
        query: &Query,
        app: &warpui::AppContext,
    ) -> Result<Vec<QueryResult<Self::Action>>, DataSourceRunErrorWrapper> {
        if !query.text.is_empty() {
            return Ok(vec![]);
        }

        Ok(self
            .slash_command_data_source
            .as_ref(app)
            .ordered_zero_state_commands(app)
            .into_iter()
            .map(Into::into)
            .collect())
    }
}
