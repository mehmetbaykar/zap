use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use ai::index::{Outline, build_outline};
use async_channel::Sender;
use futures::stream::AbortHandle;
use repo_metadata::repositories::{DetectedRepositories, DetectedRepositoriesEvent};
use repo_metadata::repository::{
    BufferingRepositorySubscriber, RepositorySubscriber, SubscriberId,
};
use repo_metadata::{Repository, RepositoryUpdate, RepositoryWatchMode};
use settings::Setting as _;
use warp_core::features::FeatureFlag;
use warpui::{Entity, ModelContext, ModelHandle, SingletonEntity};

use super::OutlineStatus;
use crate::settings::{InputSettings, InputSettingsChangedEvent};
use crate::terminal::view::TerminalView;

/// State for a repository outline, containing both the repository handle and the outline status.
#[derive(Debug)]
struct OutlineState {
    /// Handle to the repository model.
    repository: ModelHandle<Repository>,
    /// Current status of the outline.
    status: OutlineStatus,
    /// Subscriber ID for repository updates (if watching).
    subscriber_id: Option<SubscriberId>,
    /// Keep update order so moves and deletes are not lost while parsing is in flight.
    pending_updates: Vec<RepositoryUpdate>,
}

pub enum RepoOutlinesEvent {
    OutlinesUpdated(PathBuf),
}

const MAX_REPO_FILE_SIZE_LIMIT: usize = 5000;
const REPO_WATCHER_DEBOUNCE_DURATION: Duration = Duration::from_secs(10);

pub struct RepoOutlines {
    outlines: HashMap<PathBuf, OutlineState>,
    /// Queue of paths to be scanned for git repo outlines.
    outline_queue: VecDeque<PathBuf>,
    /// An `AbortHandle` for the active outline computation task.
    active_outline_task: Option<AbortHandle>,
    indexing_enabled: bool,
    /// Reject results from work started before the user disabled symbol context.
    generation: u64,
}

impl RepoOutlines {
    pub fn new(ctx: &mut ModelContext<Self>) -> Self {
        Self::new_with_indexing_enabled(true, ctx)
    }

    pub fn new_with_indexing_enabled(indexing_enabled: bool, ctx: &mut ModelContext<Self>) -> Self {
        if indexing_enabled
            && !cfg!(any(
                test,
                feature = "fast_dev",
                feature = "integration_tests"
            ))
        {
            ctx.subscribe_to_model(&DetectedRepositories::handle(ctx), |me, _, event, ctx| {
                let DetectedRepositoriesEvent::DetectedGitRepo {
                    repository,
                    source: _,
                } = event;
                me.index_repo(repository.clone(), ctx);
            });
        }

        ctx.subscribe_to_model(&InputSettings::handle(ctx), |me, _, event, ctx| {
            if let InputSettingsChangedEvent::OutlineCodebaseSymbolsForAtContextMenu { .. } = event
            {
                me.handle_setting_change_event(ctx);
            }
        });

        Self {
            outlines: Default::default(),
            outline_queue: Default::default(),
            active_outline_task: None,
            indexing_enabled,
            generation: 0,
        }
    }

    #[cfg(any(test, feature = "integration_tests"))]
    pub fn new_for_test(ctx: &mut ModelContext<Self>) -> Self {
        Self::new_with_indexing_enabled(true, ctx)
    }

    fn index_repo(&mut self, repository: ModelHandle<Repository>, ctx: &mut ModelContext<Self>) {
        let Some(repo_path) = repository.as_ref(ctx).root_dir().to_local_path() else {
            return;
        };
        if !self.outlines.contains_key(&repo_path) && self.should_build_outlines(ctx) {
            let outline_state = OutlineState {
                repository: repository.clone(),
                status: OutlineStatus::Pending,
                subscriber_id: None,
                pending_updates: Vec::new(),
            };
            self.outlines.insert(repo_path.clone(), outline_state);
            self.start_repository_subscription(&repository, repo_path, ctx);
        }
    }

    fn should_build_outlines(&self, ctx: &ModelContext<Self>) -> bool {
        self.indexing_enabled
            && FeatureFlag::AIContextMenuCode.is_enabled()
            && *InputSettings::as_ref(ctx)
                .outline_codebase_symbols_for_at_context_menu
                .value()
    }

    fn handle_setting_change_event(&mut self, ctx: &mut ModelContext<Self>) {
        if self.should_build_outlines(ctx) {
            let mut working_directories = HashSet::<PathBuf>::new();
            for window_id in ctx.window_ids() {
                for terminal_view in ctx
                    .views_of_type::<TerminalView>(window_id)
                    .into_iter()
                    .flatten()
                {
                    if let Some(dir) = terminal_view.as_ref(ctx).pwd() {
                        working_directories.insert(dir.into());
                    }
                }
            }
            for dir in working_directories {
                if let Some(repository) =
                    DetectedRepositories::as_ref(ctx).get_local_watched_repo_for_path(&dir, ctx)
                {
                    self.index_repo(repository, ctx);
                }
            }
        } else {
            self.generation += 1;
            if let Some(task) = self.active_outline_task.take() {
                task.abort();
            }
            self.outline_queue.clear();
            for (repo_path, mut state) in self.outlines.drain() {
                if let Some(subscriber_id) = state.subscriber_id.take() {
                    state.repository.update(ctx, |repo, ctx| {
                        repo.stop_watching(subscriber_id, ctx);
                    });
                }
                ctx.emit(RepoOutlinesEvent::OutlinesUpdated(repo_path));
            }
        }
    }

    /// Returns the `OutlineStatus` for the given path, if any.
    pub fn get_outline(&self, path: &Path) -> Option<(&OutlineStatus, PathBuf)> {
        let canonicalized_path = dunce::canonicalize(path).ok()?;
        self.get_outline_internal(&canonicalized_path)
    }

    /// Looks up a detected repository root without falling back to a containing repository.
    pub fn get_outline_for_repo(&self, repo_path: &Path) -> Option<&OutlineStatus> {
        let repo_path = dunce::canonicalize(repo_path).ok()?;
        self.outlines.get(&repo_path).map(|state| &state.status)
    }

    /// Returns the `OutlineStatus` for the given path, if any. The input path has to be canonicalized.
    fn get_outline_internal(&self, path: &Path) -> Option<(&OutlineStatus, PathBuf)> {
        let mut path = path.to_owned();
        loop {
            if let Some(outline_state) = self.outlines.get(&path) {
                return Some((&outline_state.status, path));
            }
            if !path.pop() {
                break;
            }
        }
        None
    }

    pub fn is_directory_indexed(&self, directory: &Path) -> bool {
        self.get_outline(directory)
            .is_some_and(|(status, _)| matches!(status, OutlineStatus::Complete(_)))
    }

    /// Computes the outline for the repo containing the next path in the queue, if any.
    fn compute_next_outline(&mut self, ctx: &mut ModelContext<Self>) {
        if self.should_build_outlines(ctx)
            && self.active_outline_task.is_none()
            && let Some(repo_root) = self.outline_queue.pop_front()
        {
            self.compute_outline_for_repo(repo_root, ctx);
        }
    }

    /// Computes the outline for the repo with the given canonical root path.
    fn compute_outline_for_repo(&mut self, repo_root: PathBuf, ctx: &mut ModelContext<Self>) {
        let root_path_clone = repo_root.clone();
        let generation = self.generation;
        let scan_abort_handle = ctx
            .spawn(
                async move { build_outline(&repo_root, Some(MAX_REPO_FILE_SIZE_LIMIT)).await },
                move |me, res, ctx| {
                    if generation != me.generation {
                        return;
                    }
                    if me.should_build_outlines(ctx) {
                        match res {
                            Ok(outline) => me.finish_outline(root_path_clone, outline, ctx),
                            Err(err) => {
                                log::warn!("Failed to construct local symbols outline: {err}");
                                if let Some(state) = me.outlines.get_mut(&root_path_clone) {
                                    state.status = OutlineStatus::Failed;
                                    state.pending_updates.clear();
                                }
                                ctx.emit(RepoOutlinesEvent::OutlinesUpdated(root_path_clone));
                            }
                        }
                    }
                    me.active_outline_task = None;
                    me.compute_next_outline(ctx);
                },
            )
            .abort_handle();
        self.active_outline_task = Some(scan_abort_handle);
    }

    fn start_repository_subscription(
        &mut self,
        repository_handle: &ModelHandle<Repository>,
        repo_path: PathBuf,
        ctx: &mut ModelContext<Self>,
    ) {
        let (repository_update_tx, repository_update_rx) = async_channel::unbounded();
        let start = repository_handle.update(ctx, |repo, ctx| {
            let inner = OutlineRepositorySubscriber {
                repository_update_tx,
            };
            let debounced =
                BufferingRepositorySubscriber::new(inner, REPO_WATCHER_DEBOUNCE_DURATION);
            repo.start_watching(
                RepositoryWatchMode::FilesystemOnly,
                Box::new(debounced),
                ctx,
            )
        });
        let subscriber_id = start.subscriber_id;

        // Store subscriber id so callers can always unsubscribe.
        if let Some(state) = self.outlines.get_mut(&repo_path) {
            state.subscriber_id = Some(subscriber_id);
        }

        let repo_path_for_cleanup = repo_path.clone();
        let repository_handle_for_cleanup = repository_handle.downgrade();
        let generation = self.generation;
        ctx.spawn(start.registration_future, move |me, res, ctx| {
            if generation != me.generation {
                return;
            }
            if let Err(err) = res {
                log::warn!("Failed to start watching repository for outline updates: {err}");
                if let Some(repository_handle) = repository_handle_for_cleanup.upgrade(ctx) {
                    repository_handle.update(ctx, |repo, ctx| {
                        repo.stop_watching(subscriber_id, ctx);
                    });
                }
                if let Some(state) = me.outlines.get_mut(&repo_path_for_cleanup) {
                    state.subscriber_id = None;
                    state.status = OutlineStatus::Failed;
                    state.pending_updates.clear();
                }
                ctx.emit(RepoOutlinesEvent::OutlinesUpdated(repo_path_for_cleanup));
                return;
            }
            // Watch registration must finish before traversal so edits made during parsing are buffered.
            if me.should_build_outlines(ctx) && me.outlines.contains_key(&repo_path_for_cleanup) {
                me.outline_queue.push_back(repo_path_for_cleanup);
                me.compute_next_outline(ctx);
            }
        });

        ctx.spawn_stream_local(
            repository_update_rx,
            move |me, update: RepositoryUpdate, ctx| {
                if generation == me.generation {
                    me.handle_repository_update(&repo_path, update, ctx);
                }
            },
            |_, _| {},
        );
    }

    fn finish_outline(
        &mut self,
        repo_path: PathBuf,
        mut outline: Outline,
        ctx: &mut ModelContext<Self>,
    ) {
        let Some(state) = self.outlines.get_mut(&repo_path) else {
            return;
        };
        let updates = std::mem::take(&mut state.pending_updates);
        if updates.is_empty() {
            state.status = OutlineStatus::Complete(outline);
            ctx.emit(RepoOutlinesEvent::OutlinesUpdated(repo_path));
            return;
        }

        state.status = OutlineStatus::Pending;
        let generation = self.generation;
        ctx.spawn(
            async move {
                for update in updates {
                    outline.update(update).await;
                }
                outline
            },
            move |me, outline, ctx| {
                if generation == me.generation && me.should_build_outlines(ctx) {
                    me.finish_outline(repo_path, outline, ctx);
                }
            },
        );
    }

    fn handle_repository_update(
        &mut self,
        repo_path: &Path,
        update: RepositoryUpdate,
        ctx: &mut ModelContext<Self>,
    ) {
        if update.is_empty() {
            return;
        }
        let Some(state) = self.outlines.get_mut(repo_path) else {
            return;
        };
        match std::mem::replace(&mut state.status, OutlineStatus::Pending) {
            OutlineStatus::Complete(outline) => {
                state.pending_updates.push(update);
                self.finish_outline(repo_path.to_owned(), outline, ctx);
            }
            OutlineStatus::Pending => state.pending_updates.push(update),
            OutlineStatus::Failed => {
                state.status = OutlineStatus::Failed;
                log::warn!("Failed to update repo outline: initial outline construction failed");
            }
        }
    }
}

impl Entity for RepoOutlines {
    type Event = RepoOutlinesEvent;
}

impl SingletonEntity for RepoOutlines {}

struct OutlineRepositorySubscriber {
    repository_update_tx: Sender<RepositoryUpdate>,
}

impl RepositorySubscriber for OutlineRepositorySubscriber {
    fn on_scan(
        &mut self,
        _repository: &Repository,
        _ctx: &mut ModelContext<Repository>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        // The model starts its initial build after watch registration, with updates already buffered.
        Box::pin(async {})
    }

    fn on_files_updated(
        &mut self,
        _repository: &Repository,
        update: &RepositoryUpdate,
        _ctx: &mut ModelContext<Repository>,
    ) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + 'static>> {
        let tx = self.repository_update_tx.clone();
        let update = update.clone();
        Box::pin(async move {
            let _ = tx.send(update).await;
        })
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
