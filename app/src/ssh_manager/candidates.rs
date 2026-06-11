//! View model for the "Candidates" area — flattens the result of
//! `warp_ssh_manager::load_candidates()` (plus the set of already-imported
//! aliases and the collapsed state) into a UI-friendly [`CandidateRow`] list.
//!
//! Design notes (matching `specs/gh-110-ssh-config-import/{PRODUCT,TECH}.md`):
//!
//! - `rows()` is a **pure function**: it only depends on the view-model's
//!   current fields, never touches IO / runtime, so unit tests can construct a
//!   `CandidatesViewModel` directly and assert on the output. This is exactly
//!   the point raised in the TDD discussion — the PR 2 warpui rendering layer is
//!   too expensive to test, so pulling out the "which rows should show" logic
//!   into a unit test is enough to cover the key decisions.
//! - `refresh()` synchronously calls `warp_ssh_manager::load_candidates()`
//!   (a <10KB file, see the tradeoff in TECH.md §3.1) and stores the result in
//!   `state`.
//! - `on_tree_changed()` is called by the panel after it subscribes to
//!   `SshTreeChangedNotifier` — it collects the `host` field of every server in
//!   the saved tree into a `HashSet`, used as the basis for the "Added" badge
//!   (PRODUCT.md decision E).
//! - "Already imported" is decided by `host == alias`. The import logic on the
//!   panel side sets `server.host` to the candidate alias (PRODUCT.md decision
//!   I), so the comparison semantics here match the import semantics.
//!
//! All fields are `pub(crate)`, visible only to `panel.rs`; `CandidatesViewModel`
//! itself is exposed via `pub` through the re-export in `mod.rs`.

use std::collections::HashSet;

use settings::Setting;
use warp_ssh_manager::{load_candidates, LoadOutcome, LoadResult, SshConfigCandidate};
use warpui::{Entity, ModelContext, SingletonEntity};

use crate::settings::SshSettings;

/// Source + status view of a one-line candidate server from `~/.ssh/config` in the UI.
pub struct CandidatesViewModel {
    /// The most recent load result. `None` means the model was just created and
    /// no refresh has been triggered yet.
    state: Option<LoadResult>,
    /// The set of `host` fields of every server in the saved tree. `rows()` uses
    /// it to decide `added`.
    added_aliases: HashSet<String>,
    /// Section collapsed state (PRODUCT.md UX table "Many candidates"). Expanded by default.
    expanded: bool,
}

impl Default for CandidatesViewModel {
    fn default() -> Self {
        Self::new()
    }
}

impl CandidatesViewModel {
    /// All-empty constructor — used when the model is first `add_model`-ed into
    /// the App. `refresh()` must be triggered by the caller at the right moment
    /// (calling it once in the panel's `new` is enough).
    pub fn new() -> Self {
        Self {
            state: None,
            added_aliases: HashSet::new(),
            expanded: true,
        }
    }

    /// Test-only constructor: stuff the internal state in explicitly, bypassing
    /// runtime / IO, to drive the various branches of `rows()` directly.
    #[cfg(test)]
    pub fn with_state(
        state: Option<LoadResult>,
        added_aliases: HashSet<String>,
        expanded: bool,
    ) -> Self {
        Self {
            state,
            added_aliases,
            expanded,
        }
    }

    /// Synchronously re-read `~/.ssh/config` and store the result in `state`.
    ///
    /// By design it returns no error — `LoadOutcome::Error` already carries the
    /// error message string back, and the UI shows it in a red error row (see
    /// the PRODUCT.md UX table "Parse / IO error").
    ///
    /// When the "auto-discover SSH hosts" setting is off, skip reading and clear the state.
    pub fn refresh(&mut self, ctx: &mut ModelContext<Self>) {
        let auto_discover = *SshSettings::as_ref(ctx).enable_ssh_auto_discovery.value();
        if !auto_discover {
            self.state = None;
            ctx.notify();
            return;
        }
        self.state = Some(load_candidates());
        ctx.notify();
    }

    /// Tree-changed callback — rebuilds `added_aliases` from the passed-in server hosts.
    ///
    /// Taking `impl IntoIterator<Item = String>` instead of `&SshRepository`
    /// lets tests avoid wiring up a real SQLite connection; the caller (panel) is
    /// responsible for collecting the host fields from `list_nodes` +
    /// `get_server` into an iterator before passing them in.
    pub fn on_tree_changed<I>(&mut self, hosts: I, ctx: &mut ModelContext<Self>)
    where
        I: IntoIterator<Item = String>,
    {
        self.added_aliases = hosts.into_iter().collect();
        ctx.notify();
    }

    /// Toggle the "section collapsed" state.
    pub fn toggle_expanded(&mut self, ctx: &mut ModelContext<Self>) {
        self.expanded = !self.expanded;
        ctx.notify();
    }

    /// Whether expanded (the panel uses this when rendering to decide whether to show body rows).
    pub fn is_expanded(&self) -> bool {
        self.expanded
    }

    /// Look up a candidate by alias — used when handling the
    /// `ImportCandidate { alias }` action; once the full fields are obtained,
    /// call `SshRepository::create_server`.
    pub fn find_candidate(&self, alias: &str) -> Option<&SshConfigCandidate> {
        let state = self.state.as_ref()?;
        match &state.outcome {
            LoadOutcome::Loaded(v) => v.iter().find(|c| c.alias == alias),
            LoadOutcome::NotFound | LoadOutcome::Error(_) => None,
        }
    }

    /// A readable string of the current `~/.ssh/config` path (for use with
    /// `notes = "Imported from {}"`). `None` means it hasn't been loaded yet, or
    /// even home couldn't be obtained.
    pub fn path_display(&self) -> Option<String> {
        self.state
            .as_ref()
            .and_then(|s| s.path.as_ref())
            .map(|p| p.display().to_string())
    }

    /// Flatten the current state into a row list — see the "pure function"
    /// convention in the module docs.
    ///
    /// Output semantics (matching the PRODUCT.md §5 UX table):
    /// - Not refreshed yet: returns an empty Vec (the panel does not render the
    ///   section when it sees `state == None`).
    /// - `NotFound`: Header + one `NotFound` row.
    /// - `Error`: Header + one `Error` row (can_refresh=true lets the user retry
    ///   after fixing the config).
    /// - `Loaded(empty)`: Header + one `Empty` row.
    /// - `Loaded(non-empty)`: Header (count = N) + N `Candidate` rows; each row's
    ///   `added` is decided by `added_aliases.contains(alias)`.
    pub fn rows(&self) -> Vec<CandidateRow> {
        let Some(state) = self.state.as_ref() else {
            return Vec::new();
        };

        let path_display = state
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();

        let mut out = Vec::new();
        let count = match &state.outcome {
            LoadOutcome::Loaded(v) => v.len(),
            LoadOutcome::NotFound | LoadOutcome::Error(_) => 0,
        };
        // Header is always the first row — even when the section is collapsed,
        // the panel still draws the header (that's the toggle entry point).
        // `can_refresh = true` always holds: any state allows the user to click
        // Refresh to re-read.
        out.push(CandidateRow::Header {
            path_display: path_display.clone(),
            count,
            can_refresh: true,
        });

        // When the section is collapsed, keep only the header and don't render the body.
        if !self.expanded {
            return out;
        }

        match &state.outcome {
            LoadOutcome::NotFound => {
                out.push(CandidateRow::NotFound { path_display });
            }
            LoadOutcome::Error(msg) => {
                out.push(CandidateRow::Error {
                    path_display,
                    message: msg.clone(),
                });
            }
            LoadOutcome::Loaded(v) if v.is_empty() => {
                out.push(CandidateRow::Empty { path_display });
            }
            LoadOutcome::Loaded(v) => {
                for c in v {
                    out.push(CandidateRow::Candidate {
                        alias: c.alias.clone(),
                        hostname: c.hostname.clone(),
                        user: c.user.clone(),
                        port: c.port,
                        identity_file: c.identity_file.as_ref().map(|p| p.display().to_string()),
                        added: self.added_aliases.contains(&c.alias),
                    });
                }
            }
        }

        out
    }
}

/// A UI-friendly row. Header is always first, followed either by a single
/// status row (NotFound / Empty / Error) or by a sequence of Candidates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CandidateRow {
    Header {
        path_display: String,
        count: usize,
        can_refresh: bool,
    },
    NotFound {
        path_display: String,
    },
    Empty {
        path_display: String,
    },
    Error {
        path_display: String,
        message: String,
    },
    Candidate {
        alias: String,
        hostname: Option<String>,
        user: Option<String>,
        port: Option<u16>,
        identity_file: Option<String>,
        added: bool,
    },
}

impl Entity for CandidatesViewModel {
    type Event = ();
}

#[cfg(test)]
#[path = "candidates_tests.rs"]
mod tests;

// So test code doesn't have to care about PathBuf's concrete on-disk path — the
// helper builds a fixed display string from `LoadResult`. The test module also
// uses it, so it lives at the outer level for #[cfg(test)] reuse.
#[cfg(test)]
pub(crate) fn fake_load_result_loaded(path: &str, cands: Vec<SshConfigCandidate>) -> LoadResult {
    LoadResult {
        path: Some(std::path::PathBuf::from(path)),
        outcome: LoadOutcome::Loaded(cands),
    }
}

#[cfg(test)]
pub(crate) fn fake_load_result_not_found(path: &str) -> LoadResult {
    LoadResult {
        path: Some(std::path::PathBuf::from(path)),
        outcome: LoadOutcome::NotFound,
    }
}

#[cfg(test)]
pub(crate) fn fake_load_result_error(path: &str, msg: &str) -> LoadResult {
    LoadResult {
        path: Some(std::path::PathBuf::from(path)),
        outcome: LoadOutcome::Error(msg.to_string()),
    }
}
