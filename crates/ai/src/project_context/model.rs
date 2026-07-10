use anyhow::Result;
use std::cell::RefCell;
use std::collections::HashMap;
#[cfg(feature = "local_fs")]
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use warpui::{Entity, ModelContext, SingletonEntity};

use super::GlobalRules;

/// The default rule file list. Order = priority (earlier ones take precedence); when multiple files
/// exist in the same directory, `RuleAtPath::respected_rule()` takes only the highest-priority one.
///
/// - WARP.md  the project's native convention.
/// - AGENTS.md community-standard (recognized by opencode / Cursor / Cline etc.).
/// - CLAUDE.md Claude Code's native convention, making projects migrated from Claude Code work out of the box.
///
/// Adding a new name only requires adjusting this array (insertion position = priority); `RuleAtPath`
/// is implemented with a priority-indexed slot array and does not need any if-else changes.
///
/// Defined outside the `cfg_if` so that paths not compiling `local_fs` (WASM / tests) can also reference it.
pub(crate) const RULES_FILE_PATTERN: &[&str] = &["WARP.md", "AGENTS.md", "CLAUDE.md"];

cfg_if::cfg_if! {
    if #[cfg(feature = "local_fs")] {
        use async_channel::Sender;
        use ignore::gitignore::Gitignore;
        use repo_metadata::entry::{Entry, FileMetadata};
        use repo_metadata::repository::RepositorySubscriber;
        use repo_metadata::{DirectoryWatcher, Repository, RepositoryUpdate};
        // `instant::Instant` is this repo's globally agreed cross-platform (including WASM) clock source, replacing
        // `std::time::Instant`. Enforced via disallowed_types in `clippy.toml`.
        use instant::Instant;
        use std::time::{Duration, SystemTime};

        const MAX_SCAN_DEPTH: usize = 3;
        const MAX_FILES_TO_SCAN: usize = 5000;

        // —— Fast-path (aligned with the opencode `findUp` pattern) ——
        //
        // Main use: after cd-ing into a new git repository, within the time window before the async
        // `index_and_store_rules` completes, `pending_context` synchronously calls this fast-path to directly stat + read the rule
        // files of cwd and its ancestor directories, ensuring AGENTS.md / WARP.md / CLAUDE.md
        // **are not missed due to async races**.
        // Once the normal path (`find_applicable_rules`) is available, the fast-path yields and clears its cache.
        //
        // UI non-jank guarantees:
        //   - The single worst case is `MAX_WALK_DEPTH * RULES_FILE_PATTERN.len()` metadata calls
        //     + a `read_to_string` of hit files (rule files are usually a few KB; Windows NTFS < 1ms/file).
        //   - `FAST_PATH_BUDGET` is a hard time-budget cutoff; on timeout it immediately returns what was collected and never blocks.
        //   - Steady-state hits (no directory changes) only do stat, not re-read files; any change in mtime / size / parent-dir-mtime
        //     triggers a re-scan.
        const MAX_WALK_DEPTH: usize = 6;
        const FAST_PATH_BUDGET: Duration = Duration::from_millis(20);
    }
}

/// Fast-path cache entry. `stamps` records the (path, mtime, size) of hit files,
/// `walked_dir_stamps` records the (path, mtime) of traversed directories, used to detect the
/// two invalidation cases of "a rule file was added / deleted / modified in a directory". The `negative` cache means
/// the last scan found no rules at all, so subsequent identical stamps are reused directly without any IO.
#[cfg(feature = "local_fs")]
#[derive(Clone, Debug)]
struct FastPathEntry {
    rules: Vec<ProjectRule>,
    /// The fast-path "root" — takes the **first-level hit** directory; on a total miss takes cwd itself.
    /// Used to construct `ProjectRulesResult.root_path`, with semantics aligned to `find_applicable_rules`.
    root_path: PathBuf,
    stamps: Vec<(PathBuf, SystemTime, u64)>,
    walked_dir_stamps: Vec<(PathBuf, SystemTime)>,
}

#[derive(Debug, Default, Clone)]
pub struct ProjectRule {
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Default, Clone)]
struct RuleAtPath {
    parent_path: PathBuf,
    warp_md: Option<ProjectRule>,
    agents_md: Option<ProjectRule>,
    claude_md: Option<ProjectRule>,
}

impl RuleAtPath {
    fn respected_rule(&self) -> Option<&ProjectRule> {
        self.warp_md
            .as_ref()
            .or(self.agents_md.as_ref())
            .or(self.claude_md.as_ref())
    }
}

#[derive(Debug, Default, Clone)]
pub struct ProjectRulesResult {
    pub root_path: PathBuf,
    pub active_rules: Vec<ProjectRule>,
    pub additional_rule_paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectRulePath {
    pub path: PathBuf,
    pub project_root: PathBuf,
}

struct FindRulesResult {
    /// Rules that are active and should be eagerly applied.
    active_rules: Vec<ProjectRule>,
    /// Rule paths that are currently not active but available to be applied if
    /// a file under its directory is edited.
    available_rule_paths: Vec<String>,
}

#[cfg(feature = "local_fs")]
fn matches_rules_pattern(file_name_str: &str) -> bool {
    for pattern in RULES_FILE_PATTERN {
        if file_name_str.to_lowercase() == pattern.to_lowercase() {
            return true;
        }
    }
    false
}

#[derive(Debug, Default, Clone)]
struct ProjectRules {
    rules: Vec<RuleAtPath>,
}

impl ProjectRules {
    /// Finds the set of rules that are active in the given path and the set that are available to be applied.
    fn find_active_or_applicable_rules(&self, path: &Path) -> FindRulesResult {
        let mut active_rules = Vec::new();
        let mut available_rule_paths = Vec::new();

        // Collect all applicable rules (rules in directories that are ancestors of the target path)
        for rule in &self.rules {
            if let Some(respected_rule) = rule.respected_rule() {
                // Check if the rule's directory is an ancestor of or equal to the target path
                if path.starts_with(&rule.parent_path) {
                    active_rules.push(respected_rule.clone());
                } else {
                    available_rule_paths.push(respected_rule.path.to_string_lossy().to_string());
                }
            }
        }

        FindRulesResult {
            active_rules,
            available_rule_paths,
        }
    }

    /// Remove a rule from the set of project rules. This returns the removed rule.
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    fn remove_rule(&mut self, path: &Path) -> Option<ProjectRule> {
        let parent = path.parent()?;
        let file_name = path.file_name().and_then(|name| name.to_str())?;

        let rule = self
            .rules
            .iter_mut()
            .find(|rule| rule.parent_path == parent)?;

        if file_name.eq_ignore_ascii_case("warp.md") {
            rule.warp_md.take()
        } else if file_name.eq_ignore_ascii_case("agents.md") {
            rule.agents_md.take()
        } else if file_name.eq_ignore_ascii_case("claude.md") {
            rule.claude_md.take()
        } else {
            None
        }
    }

    /// Upsert a rule to the set of project rules. This will create a new RuleAtPath entry if none exists and update the existing one
    /// otherwise.
    #[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
    fn upsert_rule(&mut self, path: &Path, content: String) {
        let Some(parent) = path.parent() else {
            return;
        };
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            return;
        };

        let existing_rule = self
            .rules
            .iter_mut()
            .find(|rule| rule.parent_path == parent);

        let rule_file = Some(ProjectRule {
            path: path.to_path_buf(),
            content,
        });

        match existing_rule {
            Some(rule) => {
                if file_name.eq_ignore_ascii_case("warp.md") {
                    rule.warp_md = rule_file;
                } else if file_name.eq_ignore_ascii_case("agents.md") {
                    rule.agents_md = rule_file;
                } else if file_name.eq_ignore_ascii_case("claude.md") {
                    rule.claude_md = rule_file;
                }
            }
            None => {
                let mut rule = RuleAtPath {
                    parent_path: parent.to_path_buf(),
                    ..Default::default()
                };
                if file_name.eq_ignore_ascii_case("warp.md") {
                    rule.warp_md = rule_file;
                } else if file_name.eq_ignore_ascii_case("agents.md") {
                    rule.agents_md = rule_file;
                } else if file_name.eq_ignore_ascii_case("claude.md") {
                    rule.claude_md = rule_file;
                }
                self.rules.push(rule);
            }
        };
    }
}

/// Singleton model that keeps track of mapping between paths and rule files
/// Currently supports WARP.md files, but designed to be extensible
#[cfg_attr(not(feature = "local_fs"), allow(dead_code))]
#[derive(Debug, Default)]
pub struct ProjectContextModel {
    /// Mapping from directory path to list of rule files found in that directory
    path_to_rules: HashMap<PathBuf, ProjectRules>,
    /// Fast-path synchronous rule cache (aligned with the opencode `findUp` pattern).
    ///
    /// Used as a fallback only when `find_applicable_rules` returns None (async indexing not ready / not under an indexed root),
    /// to avoid missing AGENTS.md / WARP.md injection when an AI request is sent immediately after cd.
    /// Single-threaded access (WarpUI Singleton on the main thread), so it uses `RefCell` rather than a lock,
    /// to fit the `&self` call shape of `pending_context(&self, app: &AppContext)`.
    #[cfg(feature = "local_fs")]
    fast_path_cache: RefCell<HashMap<PathBuf, FastPathEntry>>,
    /// Queued repository updates for paths that have an in-flight processing task.
    /// The presence of a key indicates an active async task for that path;
    /// the Vec holds updates that arrived while that task was running.
    #[cfg(feature = "local_fs")]
    pending_updates: HashMap<PathBuf, Vec<RepositoryUpdate>>,
    /// Repo roots that already have a watcher registered, so we never
    /// subscribe more than once per root.
    #[cfg(feature = "local_fs")]
    watched_roots: HashSet<PathBuf>,
    /// File-based global rules and their local watcher state. Kept separate
    /// from `path_to_rules`, which is project-scoped.
    pub(super) global_rules: GlobalRules,
}

#[derive(Default, Debug)]
pub struct RulesDelta {
    pub discovered_rules: Vec<ProjectRulePath>,
    pub deleted_rules: Vec<PathBuf>,
}

impl RulesDelta {
    /// Merge another delta into this one, preserving the ordering of operations.
    ///
    /// When the same path appears across sequential deltas the *last* operation
    /// wins. For example:
    ///   - (add A, delete A) → net effect is **delete**
    ///   - (delete A, add A) → net effect is **add**
    ///
    /// This is important because consumers (e.g. persistence) apply the delta
    /// incrementally; a symmetric "cancel both sides" approach would silently
    /// drop real state changes.
    #[cfg(any(feature = "local_fs", test))]
    fn merge(&mut self, other: RulesDelta) {
        // Each newly-discovered path supersedes any prior deletion or earlier
        // discovery of the same path.
        for discovered in &other.discovered_rules {
            self.deleted_rules.retain(|p| *p != discovered.path);
            self.discovered_rules.retain(|r| r.path != discovered.path);
        }
        // Each newly-deleted path supersedes any prior discovery or earlier
        // deletion of the same path.
        for deleted in &other.deleted_rules {
            self.discovered_rules.retain(|r| r.path != *deleted);
            self.deleted_rules.retain(|p| *p != *deleted);
        }
        self.discovered_rules.extend(other.discovered_rules);
        self.deleted_rules.extend(other.deleted_rules);
    }
}

#[derive(Default, Debug)]
pub struct GlobalRulesDelta {
    pub discovered_rules: Vec<PathBuf>,
    pub deleted_rules: Vec<PathBuf>,
}

/// Events emitted by the ProjectContextModel
pub enum ProjectContextModelEvent {
    /// Emitted when a path has been indexed
    PathIndexed,
    /// Emitted when the known set of rule files changed
    KnownRulesChanged(RulesDelta),
    /// Emitted when the set of indexed global rule files changed
    GlobalRulesChanged(GlobalRulesDelta),
}

impl ProjectContextModel {
    #[cfg_attr(not(feature = "local_fs"), allow(unused_variables))]
    pub fn new_from_persisted(
        persisted_rules: Vec<ProjectRulePath>,
        ctx: &mut ModelContext<Self>,
    ) -> Self {
        #[cfg(feature = "local_fs")]
        ctx.spawn(
            async move { Self::read_persisted_rules(persisted_rules).await },
            |me, mut res, ctx| {
                // Zap: this originally called `try_initialize_and_register_watcher` for each persisted root,
                // which internally went through
                // `DetectedRepositories::detect_possible_git_repo(ProjectRulesIndexing)`
                // to fire an event making RepoMetadataModel fully index the 6 persisted repositories
                // (the biggest cold-start background CPU cost for Zap BYOP).
                //
                // Now it only fills the in-memory path_to_rules cache and does not actively fire
                // detect events. When the user later cd's into a repository via the terminal,
                // RepoDetectionSource::TerminalNavigation naturally triggers an independent detect,
                // which then goes through register_watcher_for_path.
                //
                // Practical impact: persisted rules are not watched in real time until the user enters that repository.
                // The cache itself is still usable, so AI rule lookups are unaffected.
                res.extend(me.path_to_rules.drain());
                me.path_to_rules = res;
                ctx.emit(ProjectContextModelEvent::PathIndexed);
            },
        );

        Self::default()
    }

    /// Index a path and find all rule files from that path up to the root directory
    /// Returns a list of all rule files found
    #[cfg_attr(not(feature = "local_fs"), allow(unused_variables))]
    pub fn index_and_store_rules(
        &mut self,
        root_path: PathBuf,
        ctx: &mut ModelContext<Self>,
    ) -> Result<()> {
        #[cfg(feature = "local_fs")]
        {
            let root_clone = root_path.clone();

            ctx.spawn(
                async move { Self::scan_directory_for_rules(&root_path).await },
                move |me, res: Result<ProjectRules>, ctx| match res {
                    Ok(rule_files) => {
                        me.register_watcher_for_path(&root_clone, ctx);

                        // Persist the discovered rules.
                        let delta = RulesDelta {
                            discovered_rules: rule_files
                                .rules
                                .iter()
                                .filter_map(|rule| {
                                    rule.warp_md.as_ref().map(|rule| ProjectRulePath {
                                        project_root: root_clone.clone(),
                                        path: rule.path.clone(),
                                    })
                                })
                                .chain(rule_files.rules.iter().filter_map(|rule| {
                                    rule.agents_md.as_ref().map(|rule| ProjectRulePath {
                                        project_root: root_clone.clone(),
                                        path: rule.path.clone(),
                                    })
                                }))
                                .collect(),
                            deleted_rules: Default::default(),
                        };
                        ctx.emit(ProjectContextModelEvent::KnownRulesChanged(delta));

                        me.path_to_rules.insert(root_clone, rule_files);
                        ctx.emit(ProjectContextModelEvent::PathIndexed);
                    }
                    Err(e) => log::warn!(
                        "Couldn't index rules for path {}: {}",
                        root_clone.display(),
                        e
                    ),
                },
            );
        }

        Ok(())
    }

    // Zap: the original `try_initialize_and_register_watcher` was the entry point that, when starting from a persisted rule path,
    // forcibly detected the repo, with startup then going through RepoMetadataModel full indexing. It has been removed along with
    // the detect call in `new_from_persisted`; now register_watcher_for_path is only reached passively via the
    // `RepoDetectionSource::TerminalNavigation` path triggered by terminal cd.

    #[cfg(feature = "local_fs")]
    fn register_watcher_for_path(&mut self, path: &Path, ctx: &mut ModelContext<Self>) {
        if self.watched_roots.contains(path) {
            return;
        }

        let Some(repository_model) =
            DirectoryWatcher::as_ref(ctx).get_watched_directory_for_path(path)
        else {
            return;
        };

        self.watched_roots.insert(path.to_path_buf());

        let (repository_update_tx, repository_update_rx) = async_channel::unbounded();
        let start = repository_model.update(ctx, |repo, ctx| {
            repo.start_watching(
                Box::new(ProjectContextRepositorySubscriber {
                    repository_update_tx,
                }),
                ctx,
            )
        });

        let subscriber_id = start.subscriber_id;
        let repository_model_for_cleanup = repository_model.downgrade();
        let path_clone = path.to_path_buf();
        let path_for_log = path_clone.clone();
        ctx.spawn(start.registration_future, move |_, res, ctx| {
            if let Err(err) = res {
                log::warn!(
                    "Failed to start watching repository for rule updates at {}: {err}",
                    path_for_log.display()
                );

                if let Some(repository_model) = repository_model_for_cleanup.upgrade(ctx) {
                    repository_model.update(ctx, |repo, ctx| {
                        repo.stop_watching(subscriber_id, ctx);
                    });
                }
            }
        });

        ctx.spawn_stream_local(
            repository_update_rx.clone(),
            move |me, update, ctx| {
                if update.is_empty() {
                    return;
                }

                // If there's already an in-flight task for this path, queue the update
                // instead of spawning a concurrent task that could overwrite results.
                if let Some(queued) = me.pending_updates.get_mut(&path_clone) {
                    queued.push(update);
                    return;
                }

                let Some(rules) = me.path_to_rules.get(&path_clone).cloned() else {
                    return;
                };

                // Mark this path as having an in-flight task (empty queue).
                me.pending_updates.insert(path_clone.clone(), Vec::new());

                let repo_path = path_clone.clone();
                let repo_path_for_closure = repo_path.clone();
                ctx.spawn(
                    async move { Self::process_repository_updates(update, rules, repo_path).await },
                    move |me, (rules, rule_delta), ctx| {
                        me.apply_update_result(&repo_path_for_closure, rules, rule_delta, ctx);
                    },
                );
            },
            |_, _| {},
        );
    }

    /// Called when an async update task completes: emits events, stores the new rules,
    /// and drains any updates that queued up while the task was in flight.
    #[cfg(feature = "local_fs")]
    fn apply_update_result(
        &mut self,
        path: &Path,
        rules: ProjectRules,
        rule_delta: RulesDelta,
        ctx: &mut ModelContext<Self>,
    ) {
        ctx.emit(ProjectContextModelEvent::KnownRulesChanged(rule_delta));
        self.path_to_rules.insert(path.to_path_buf(), rules);
        self.drain_pending_updates(path, ctx);
        ctx.emit(ProjectContextModelEvent::PathIndexed);
    }

    /// Processes any queued updates for a path after the previous async task completes.
    /// Each batch runs sequentially against the latest rules, preventing stale-snapshot races.
    #[cfg(feature = "local_fs")]
    fn drain_pending_updates(&mut self, path: &Path, ctx: &mut ModelContext<Self>) {
        let path_buf = path.to_path_buf();
        let Some(queued) = self.pending_updates.get_mut(&path_buf) else {
            return;
        };

        if queued.is_empty() {
            self.pending_updates.remove(&path_buf);
            return;
        }

        let updates = std::mem::take(queued);
        let Some(rules) = self.path_to_rules.get(&path_buf).cloned() else {
            self.pending_updates.remove(&path_buf);
            return;
        };

        let repo_path = path_buf.clone();
        let repo_path_for_closure = path_buf;
        ctx.spawn(
            async move {
                let mut current_rules = rules;
                let mut combined_delta = RulesDelta::default();
                for update in updates {
                    let (updated_rules, delta) =
                        Self::process_repository_updates(update, current_rules, repo_path.clone())
                            .await;
                    current_rules = updated_rules;
                    combined_delta.merge(delta);
                }
                (current_rules, combined_delta)
            },
            move |me, (rules, rule_delta), ctx| {
                me.apply_update_result(&repo_path_for_closure, rules, rule_delta, ctx);
            },
        );
    }

    /// Index all configured global rule sources.
    ///
    /// `ProjectContextModel` remains the public rule-context facade; the
    /// global source registry, cache, and watcher plumbing live in
    /// `global_rules`.
    pub fn index_global_rules(&mut self, ctx: &mut ModelContext<Self>) {
        self.global_rules.index(ctx);
    }

    /// Project-only rule lookup. Returns `Some` only when an indexed project
    /// root above `path` actually contributes a rule — globals are
    /// deliberately ignored.
    ///
    /// Use this for callers that need a project-initialization signal rather
    /// than the full rule context sent to agents.
    pub fn find_applicable_project_rules(&self, path: &Path) -> Option<ProjectRulesResult> {
        let mut current_path = path.to_owned();

        // Walk upwards from `path` toward the filesystem root, stopping at the
        // first directory we have indexed project rules for. `path_to_rules`
        // is keyed by indexed project root, so popping the path produces
        // every ancestor directory until we hit a known root or `pop()`
        // returns false (we've reached the top of the path).
        loop {
            if let Some(rules) = self.path_to_rules.get(&current_path) {
                let result = rules.find_active_or_applicable_rules(path);
                if result.active_rules.is_empty() && result.available_rule_paths.is_empty() {
                    return None;
                }
                return Some(ProjectRulesResult {
                    root_path: current_path,
                    active_rules: result.active_rules,
                    additional_rule_paths: result.available_rule_paths,
                });
            }

            if !current_path.pop() {
                return None;
            }
        }
    }

    /// Returns the rules applicable to `path`, layering global rules on top of
    /// any project rules discovered up the directory tree.
    ///
    /// Precedence is `global > project WARP.md > project AGENTS.md`. Globals
    /// are always included (when present) regardless of project state; the
    /// existing in-directory `WARP.md > AGENTS.md` shadow inside
    /// [`RuleAtPath::respected_rule`] still applies to project rules.
    ///
    /// This is the entry point used by `BlocklistAIContextModel` when packing
    /// `AIAgentContext::ProjectRules` for an agent query. Callers that need
    /// a project-only signal should use
    /// [`Self::find_applicable_project_rules`] instead.
    pub fn find_applicable_rules(&self, path: &Path) -> Option<ProjectRulesResult> {
        let project_result = self.find_applicable_project_rules(path);

        // Layered precedence: global rules are always included alongside
        // project rules. `global_rules` is a `BTreeMap`, so iteration is
        // sorted by path — deterministic without needing a separate
        // ordering pass.
        let mut active_rules: Vec<ProjectRule> = self.global_rules.active_rules().collect();
        let (project_root, additional_rule_paths) = match project_result {
            Some(project) => {
                active_rules.extend(project.active_rules);
                (Some(project.root_path), project.additional_rule_paths)
            }
            None => (None, Vec::new()),
        };

        if active_rules.is_empty() && additional_rule_paths.is_empty() {
            return None;
        }

        // Use the indexed project root when available; otherwise fall back to
        // the parent of the first global rule (or empty).
        let root_path = project_root
            .unwrap_or_else(|| self.global_rules.first_rule_parent().unwrap_or_default());

        Some(ProjectRulesResult {
            root_path,
            active_rules,
            additional_rule_paths,
        })
    }

    /// The unified entry point for rule lookup: prefers the normal path, falling back to the synchronous fast-path when async indexing is not ready.
    ///
    /// Aligned with the `findUp` behavior of opencode `Instruction.systemPaths()` (
    /// `opencode/packages/opencode/src/session/instruction.ts`): stats rule files from cwd upward level by level,
    /// stopping at the first-level hit. The fast-path and the normal path **never coexist**:
    /// as soon as the normal path returns Some, the corresponding entry in the fast-path cache is cleared immediately, ensuring that once indexing completes
    /// all subsequent requests go through the normal path (which gets subdirectory rules + real-time watcher updates).
    #[cfg_attr(not(feature = "local_fs"), allow(unused_variables))]
    pub fn find_rules_with_fast_path(&self, cwd: &Path) -> Option<ProjectRulesResult> {
        if let Some(found) = self.find_applicable_rules(cwd) {
            #[cfg(feature = "local_fs")]
            {
                // The normal path is now available; discard the fast-path cache (to avoid later getting stale data).
                self.fast_path_cache.borrow_mut().remove(cwd);
            }
            return Some(found);
        }

        #[cfg(feature = "local_fs")]
        {
            return self.fast_path_lookup(cwd);
        }

        #[allow(unreachable_code)]
        None
    }

    /// Fast-path synchronous lookup + reading of the rule files of cwd and its ancestor directories. Only called when the normal path is None.
    ///
    /// Return semantics are consistent with `find_applicable_rules`:
    ///   - Some(ProjectRulesResult) with at least 1 active rule
    ///   - None means no rule was found (a negative cache is written, so subsequent identical stamps do no IO)
    #[cfg(feature = "local_fs")]
    fn fast_path_lookup(&self, cwd: &Path) -> Option<ProjectRulesResult> {
        // 1) Cache hit path: stat the stamps once, and if all match, reuse the cache (without re-reading files).
        if let Some(entry) = self.fast_path_cache.borrow().get(cwd).cloned() {
            if Self::fast_path_entry_still_valid(&entry) {
                return Self::result_from_fast_path_entry(&entry);
            }
        }

        // 2) Cache miss / invalid: synchronous scan. The `FAST_PATH_BUDGET` is a hard cutoff; the UI never janks.
        let entry = Self::scan_fast_path(cwd);
        let result = Self::result_from_fast_path_entry(&entry);
        self.fast_path_cache
            .borrow_mut()
            .insert(cwd.to_path_buf(), entry);
        result
    }

    /// Synchronously stats + reads rule files from `start` upward level by level. Aligned with opencode `findUp`,
    /// but adds the `MAX_WALK_DEPTH` + `FAST_PATH_BUDGET` dual guarantees so the UI never blocks.
    ///
    /// Each level takes the first hit per `RULES_FILE_PATTERN` (WARP.md > AGENTS.md), aligned with
    /// `RuleAtPath::respected_rule()` semantics.
    #[cfg(feature = "local_fs")]
    fn scan_fast_path(start: &Path) -> FastPathEntry {
        let deadline = Instant::now() + FAST_PATH_BUDGET;
        let mut rules = Vec::new();
        let mut stamps = Vec::new();
        let mut walked_dir_stamps = Vec::new();
        let mut first_hit_dir: Option<PathBuf> = None;
        let mut current: PathBuf = start.to_path_buf();

        for _ in 0..MAX_WALK_DEPTH {
            if Instant::now() >= deadline {
                break;
            }

            // Record the directory mtime so we can later detect the two kinds of changes "a rule file was added/deleted in the directory".
            if let Ok(meta) = std::fs::metadata(&current) {
                if let Ok(mtime) = meta.modified() {
                    walked_dir_stamps.push((current.clone(), mtime));
                }
            }

            // At this level, find the first rule file by priority. Aligned with RuleAtPath::respected_rule() semantics.
            for filename in RULES_FILE_PATTERN {
                if Instant::now() >= deadline {
                    break;
                }
                let candidate = current.join(filename);
                let Ok(meta) = std::fs::metadata(&candidate) else {
                    continue;
                };
                if !meta.is_file() {
                    continue;
                }
                let Ok(mtime) = meta.modified() else { continue };
                let size = meta.len();
                let Ok(content) = std::fs::read_to_string(&candidate) else {
                    continue;
                };
                if first_hit_dir.is_none() {
                    first_hit_dir = Some(current.clone());
                }
                rules.push(ProjectRule {
                    path: candidate.clone(),
                    content,
                });
                stamps.push((candidate, mtime, size));
                break; // take only 1 at this level
            }

            if !current.pop() {
                break;
            }
        }

        FastPathEntry {
            root_path: first_hit_dir.unwrap_or_else(|| start.to_path_buf()),
            rules,
            stamps,
            walked_dir_stamps,
        }
    }

    /// Cache invalidation check. Only stats, does not read file content.
    /// - Hit files' mtime/size unchanged → content can be reused
    /// - Traversed directories' mtime unchanged → no rule files were added/deleted
    ///
    /// With the `FAST_PATH_BUDGET` budget, a timeout during stat is treated as invalid, triggering a re-scan.
    #[cfg(feature = "local_fs")]
    fn fast_path_entry_still_valid(entry: &FastPathEntry) -> bool {
        let deadline = Instant::now() + FAST_PATH_BUDGET;
        for (path, mtime, size) in &entry.stamps {
            if Instant::now() >= deadline {
                return false;
            }
            let Ok(meta) = std::fs::metadata(path) else {
                return false;
            };
            if meta.len() != *size {
                return false;
            }
            if meta.modified().ok().as_ref() != Some(mtime) {
                return false;
            }
        }
        for (dir, mtime) in &entry.walked_dir_stamps {
            if Instant::now() >= deadline {
                return false;
            }
            let Ok(meta) = std::fs::metadata(dir) else {
                return false;
            };
            if meta.modified().ok().as_ref() != Some(mtime) {
                return false;
            }
        }
        true
    }

    /// Converts a FastPathEntry into the unified external `ProjectRulesResult`.
    /// Empty rules return None, with semantics aligned to `find_applicable_rules`.
    #[cfg(feature = "local_fs")]
    fn result_from_fast_path_entry(entry: &FastPathEntry) -> Option<ProjectRulesResult> {
        if entry.rules.is_empty() {
            return None;
        }
        Some(ProjectRulesResult {
            root_path: entry.root_path.clone(),
            active_rules: entry.rules.clone(),
            additional_rule_paths: Vec::new(),
        })
    }

    #[cfg(feature = "local_fs")]
    async fn process_repository_updates(
        repository_update: RepositoryUpdate,
        mut existing_rules: ProjectRules,
        project_root: PathBuf,
    ) -> (ProjectRules, RulesDelta) {
        let mut rules_delta = RulesDelta::default();
        // Handle deleted files - remove rules for deleted rule files
        for target_file in &repository_update.deleted {
            // Skip gitignored files
            if target_file.is_ignored {
                continue;
            }
            if let Some(file_name_str) = target_file.path.file_name().and_then(|name| name.to_str())
            {
                if matches_rules_pattern(file_name_str) {
                    // Remove the rule from existing rules
                    existing_rules.remove_rule(&target_file.path);
                    rules_delta.deleted_rules.push(target_file.path.clone());

                    log::debug!("Removed rule file: {}", target_file.path.display());
                }
            }
        }

        // Handle moved files - update paths for moved rule files
        for (to_target, from_target) in &repository_update.moved {
            // Skip gitignored files
            if to_target.is_ignored || from_target.is_ignored {
                continue;
            }
            if let Some(file_name_str) = to_target.path.file_name().and_then(|name| name.to_str()) {
                if matches_rules_pattern(file_name_str) {
                    // Find and update the rule with the old path
                    if let Some(rule) = existing_rules.remove_rule(&from_target.path) {
                        // Emit deletion event for old path
                        rules_delta.deleted_rules.push(from_target.path.clone());

                        existing_rules.upsert_rule(&to_target.path, rule.content);

                        // Emit upsert event for new path
                        rules_delta.discovered_rules.push(ProjectRulePath {
                            path: to_target.path.clone(),
                            project_root: project_root.clone(),
                        });

                        log::debug!(
                            "Updated rule file path: {} -> {}",
                            from_target.path.display(),
                            to_target.path.display()
                        );
                    }
                }
            }
        }

        // Handle added/updated files - upsert rules for rule files
        for target_file in repository_update.added_or_modified() {
            // Skip gitignored files
            if target_file.is_ignored {
                continue;
            }
            if let Some(file_name_str) = target_file.path.file_name().and_then(|name| name.to_str())
            {
                if matches_rules_pattern(file_name_str) {
                    // Read the content of the rule file
                    match async_fs::read_to_string(&target_file.path).await {
                        Ok(content) => {
                            existing_rules.upsert_rule(&target_file.path, content);
                            rules_delta.discovered_rules.push(ProjectRulePath {
                                path: target_file.path.clone(),
                                project_root: project_root.clone(),
                            });
                        }
                        Err(e) => {
                            log::warn!(
                                "Failed to read updated rule file {}: {}",
                                target_file.path.display(),
                                e
                            );
                        }
                    }
                }
            }
        }

        (existing_rules, rules_delta)
    }

    /// Scan a directory for rule files (currently WARP.md, extensible for future file types)
    /// Uses repo_metadata::entry::build_tree for efficient directory traversal
    #[cfg(feature = "local_fs")]
    async fn scan_directory_for_rules(dir_path: &Path) -> Result<ProjectRules> {
        use repo_metadata::entry::IgnoredPathStrategy;

        let mut rule_files = ProjectRules::default();

        if !async_fs::metadata(dir_path).await?.is_dir() {
            return Ok(rule_files);
        }

        // Use build_tree to collect all files, then filter for rule files
        let mut files = Vec::<FileMetadata>::new();
        let mut gitignores = Vec::<Gitignore>::new();

        // Collect patterns that should not be ignored
        let override_ignore_patterns: Vec<String> =
            RULES_FILE_PATTERN.iter().map(|s| s.to_string()).collect();
        let mut file_limit = MAX_FILES_TO_SCAN;

        // Build the file tree using repo_metadata's build_tree function
        let ignore_behavior = IgnoredPathStrategy::IncludeOnly(override_ignore_patterns.clone());

        let _ = Entry::build_tree(
            dir_path,
            &mut files,
            &mut gitignores,
            Some(&mut file_limit),
            MAX_SCAN_DEPTH,
            0,
            &ignore_behavior,
        )?;

        // Filter files to only include those matching RULES_FILE_PATTERN
        for file_metadata in files {
            let path = &file_metadata.path;
            let file_name = path.file_name();

            if let Some(file_name_str) = file_name {
                if matches_rules_pattern(file_name_str) {
                    // Read the content of the rule file
                    let local_path = file_metadata.path.to_local_path_lossy();
                    let content = match async_fs::read_to_string(&local_path).await {
                        Ok(content) => content,
                        Err(e) => {
                            log::warn!("Failed to read rule file {}: {e}", file_metadata.path,);
                            break;
                        }
                    };

                    rule_files.upsert_rule(&local_path, content);
                }
            }
        }

        Ok(rule_files)
    }

    #[cfg(feature = "local_fs")]
    async fn read_persisted_rules(
        rule_paths: Vec<ProjectRulePath>,
    ) -> HashMap<PathBuf, ProjectRules> {
        let mut rules: HashMap<PathBuf, ProjectRules> = HashMap::new();

        for rule in rule_paths {
            match async_fs::read_to_string(&rule.path).await {
                Ok(content) => {
                    let existing_rules = rules.entry(rule.project_root).or_default();
                    existing_rules.upsert_rule(&rule.path, content);
                }
                Err(e) => {
                    log::debug!(
                        "Failed to read rule file from persistence {}: {}",
                        rule.path.display(),
                        e
                    );
                    // Continue processing other files even if one fails
                }
            }
        }

        rules
    }

    pub fn indexed_rules(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.path_to_rules.values().flat_map(|rules| {
            rules.rules.iter().filter_map(|rules| {
                rules
                    .respected_rule()
                    .map(|project_rule| project_rule.path.clone())
            })
        })
    }

    /// Absolute paths of every indexed global rule file (e.g. `~/.agents/AGENTS.md`).
    /// Iteration order is sorted by path because global rules are backed by a `BTreeMap`.
    pub fn global_rule_paths(&self) -> impl Iterator<Item = PathBuf> + '_ {
        self.global_rules.paths()
    }

    /// Returns the rule file paths associated with a specific workspace root path.
    pub fn rules_for_workspace(&self, workspace_path: &Path) -> Vec<PathBuf> {
        self.path_to_rules
            .get(workspace_path)
            .into_iter()
            .flat_map(|rules| {
                rules.rules.iter().filter_map(|rule| {
                    rule.respected_rule()
                        .map(|project_rule| project_rule.path.clone())
                })
            })
            .collect()
    }
}

impl Entity for ProjectContextModel {
    type Event = ProjectContextModelEvent;
}

impl SingletonEntity for ProjectContextModel {}

#[cfg(feature = "local_fs")]
struct ProjectContextRepositorySubscriber {
    repository_update_tx: Sender<RepositoryUpdate>,
}

#[cfg(feature = "local_fs")]
impl RepositorySubscriber for ProjectContextRepositorySubscriber {
    fn on_scan(
        &mut self,
        _repository: &Repository,
        _ctx: &mut ModelContext<Repository>,
    ) -> std::pin::Pin<Box<dyn std::prelude::rust_2024::Future<Output = ()> + Send + 'static>> {
        // The model can safely ignore the initial scan because the model only subscribes
        // after the repository is already scanned.
        Box::pin(async {})
    }

    fn on_files_updated(
        &mut self,
        _repository: &Repository,
        update: &repo_metadata::RepositoryUpdate,
        _ctx: &mut ModelContext<Repository>,
    ) -> std::pin::Pin<Box<dyn std::prelude::rust_2024::Future<Output = ()> + Send + 'static>> {
        let tx = self.repository_update_tx.clone();
        let update = update.clone();
        Box::pin(async move {
            let _ = tx.send(update).await;
        })
    }
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
