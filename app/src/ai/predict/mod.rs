//! This module contains all code relevant to Agent Predict within Zap.
//!
//! Agent Predict attempts to predict the next action the user will take in Zap.

pub(crate) mod generate_ai_input_suggestions;
pub(crate) mod generate_am_query_suggestions;
pub mod next_command_model;
// Zap (Wave 3-2): the `predict_am_queries` API module is physically removed — the original `ServerApi::predict_am_queries`
// with 0 external consumers was removed in tandem; FeatureFlag::PredictAMQueries / the
// `predict_am_queries_future_handle` in terminal/input.rs are kept only as a control switch / handle alias, and the module is no longer needed.
pub mod prompt_suggestions;
