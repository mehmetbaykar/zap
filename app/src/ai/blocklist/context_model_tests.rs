//! Unit tests for [`BlocklistAIContextModel`].
//!
//! These tests use [`BlocklistAIContextModel::new_for_test`] and a small conversation-selection
//! fake to avoid unrelated subscriptions while exercising context behavior.

use std::sync::Arc;

#[cfg(feature = "local_fs")]
use ai::project_context::model::{ProjectContextModel, ProjectRule};
use parking_lot::FairMutex;
#[cfg(feature = "local_fs")]
use repo_metadata::DirectoryWatcher;
#[cfg(feature = "local_fs")]
use warp_util::host_id::HostId;
#[cfg(feature = "local_fs")]
use warp_util::local_or_remote_path::LocalOrRemotePath;
#[cfg(feature = "local_fs")]
use warp_util::remote_path::RemotePath;
#[cfg(feature = "local_fs")]
use warp_util::standardized_path::StandardizedPath;
use warpui::r#async::executor::Background;
use warpui::{App, EntityId, ModelHandle, SingletonEntity};

use super::{BlocklistAIContextModel, PendingAttachment, PendingFile};
#[cfg(feature = "local_fs")]
use crate::ai::agent::AnyFileContent;
use crate::ai::agent::conversation::AIConversationId;
use crate::ai::agent::{AIAgentAttachment, AIAgentContext, ImageContext};
use crate::ai::agent_conversations_model::{
    AgentConversationEntry, AgentConversationListEntryState, AgentConversationListPolicy,
};
use crate::ai::blocklist::agent_view::{AgentViewEntryOrigin, EnterAgentViewError};
use crate::ai::blocklist::conversation_selection::{
    ConversationSelection, ConversationSelectionEvent,
};
use crate::ai::blocklist::{
    BlocklistAIHistoryEvent, BlocklistAIHistoryModel, QueuedQuery, QueuedQueryModel,
    QueuedQueryOrigin,
};
#[cfg(feature = "local_fs")]
use crate::code_review::git_repo_model::GitRepoStatusModel;
#[cfg(feature = "local_fs")]
use crate::code_review::github_repo_model::GitHubRepoModel;
use crate::terminal::color::{self, Colors};
use crate::terminal::event_listener::ChannelEventListener;
use crate::terminal::model::test_utils::block_size;
use crate::terminal::model::{BlockId, TerminalModel};
use crate::test_util::settings::initialize_history_persistence_for_tests;
use crate::util::git::{PrInfo, RepositoryInfo};

impl BlocklistAIContextModel {
    pub(crate) fn append_pending_attachments_for_test(
        &mut self,
        attachments: Vec<PendingAttachment>,
    ) {
        self.pending_attachments.extend(attachments);
    }

    pub(crate) fn insert_pending_block_id_for_test(&mut self, block_id: BlockId) {
        self.pending_context_block_ids.insert(block_id);
    }

    pub(crate) fn set_pending_selected_text_for_test(&mut self, text: Option<String>) {
        self.pending_context_selected_text = text;
    }
}

struct TestConversationSelection {
    terminal_surface_id: EntityId,
    selected_conversation_id: Option<AIConversationId>,
}

impl TestConversationSelection {
    fn new(
        terminal_surface_id: EntityId,
        _: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) -> Self {
        Self {
            terminal_surface_id,
            selected_conversation_id: None,
        }
    }
}

impl AgentConversationListPolicy for TestConversationSelection {
    fn classify_entry(
        &self,
        _: &AgentConversationEntry,
        _: &warpui::AppContext,
    ) -> AgentConversationListEntryState {
        AgentConversationListEntryState::Unavailable
    }
}

impl ConversationSelection for TestConversationSelection {
    fn selected_conversation_id(&self, _: &warpui::AppContext) -> Option<AIConversationId> {
        self.selected_conversation_id
    }

    fn is_conversation_active(&self, _: &warpui::AppContext) -> bool {
        self.selected_conversation_id.is_some()
    }

    fn is_conversation_fullscreen(&self, _: &warpui::AppContext) -> bool {
        self.selected_conversation_id.is_some()
    }

    fn select_existing_conversation(
        &mut self,
        conversation_id: AIConversationId,
        _: AgentViewEntryOrigin,
        ctx: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) {
        if self.selected_conversation_id != Some(conversation_id) {
            self.selected_conversation_id = Some(conversation_id);
            ctx.emit(ConversationSelectionEvent::Changed);
        }
    }

    fn select_new_conversation(
        &mut self,
        _: AgentViewEntryOrigin,
        ctx: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) {
        if self.selected_conversation_id.take().is_some() {
            ctx.emit(ConversationSelectionEvent::Changed);
        }
    }

    fn try_start_new_conversation(
        &mut self,
        _: AgentViewEntryOrigin,
        ctx: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) -> Result<AIConversationId, EnterAgentViewError> {
        let conversation_id = BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
            history.start_new_conversation(self.terminal_surface_id, false, false, false, ctx)
        });
        self.select_existing_conversation(conversation_id, AgentViewEntryOrigin::Cli, ctx);
        Ok(conversation_id)
    }

    fn pending_query_autoexecute_override(
        &self,
        app: &warpui::AppContext,
    ) -> crate::ai::agent::conversation::AIConversationAutoexecuteMode {
        self.selected_conversation_id
            .as_ref()
            .and_then(|conversation_id| {
                BlocklistAIHistoryModel::as_ref(app).conversation(conversation_id)
            })
            .map(|conversation| conversation.autoexecute_override())
            .unwrap_or_default()
    }

    fn toggle_pending_query_autoexecute(
        &mut self,
        ctx: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) {
        if let Some(conversation_id) = self.selected_conversation_id {
            BlocklistAIHistoryModel::handle(ctx).update(ctx, |history, ctx| {
                history.toggle_autoexecute_override(
                    &conversation_id,
                    self.terminal_surface_id,
                    ctx,
                );
            });
        }
    }

    fn handle_history_event(
        &mut self,
        _: &BlocklistAIHistoryEvent,
        _: &mut warpui::ModelContext<Box<dyn ConversationSelection>>,
    ) {
    }
}

#[cfg(feature = "local_fs")]
#[test]
fn repository_context_reads_github_repo_model() {
    App::test((), |mut app| async move {
        let context_model = build_test_context_model(&mut app);
        let temp_dir = tempfile::TempDir::new().unwrap();
        let watcher_handle = app.add_singleton_model(DirectoryWatcher::new_for_testing);
        let repository = watcher_handle.update(&mut app, |watcher, ctx| {
            watcher
                .add_directory(
                    StandardizedPath::from_local_canonicalized(temp_dir.path()).unwrap(),
                    ctx,
                )
                .unwrap()
        });
        let git_status =
            app.add_model(move |ctx| GitRepoStatusModel::new_local_for_test(repository, None, ctx));
        let github_repo_model =
            app.add_model(move |ctx| GitHubRepoModel::new_local_for_test(git_status, ctx));

        github_repo_model.update(&mut app, |model, ctx| {
            model.set_repository_info_for_test(
                Some(RepositoryInfo {
                    name: "warp-internal".to_owned(),
                    owner: Some("warpdotdev".to_owned()),
                    host: Some("github.com".to_owned()),
                }),
                ctx,
            );
        });

        context_model.update(&mut app, |model, _| {
            model.set_github_repo_model(Some(github_repo_model.downgrade()));
        });

        context_model.read(&app, |model, ctx| {
            assert_eq!(
                model.repository_context(ctx),
                Some(AIAgentContext::Repository {
                    name: "warp-internal".to_owned(),
                    owner: Some("warpdotdev".to_owned()),
                    host: Some("github.com".to_owned()),
                })
            );
        });

        context_model.update(&mut app, |model, _| {
            model.set_github_repo_model(None);
        });

        context_model.read(&app, |model, ctx| {
            assert_eq!(model.repository_context(ctx), None);
        });
    });
}

/// Builds a [`BlocklistAIContextModel`] with stub dependencies. None of the dependencies are
/// exercised by the methods under test; they only need to satisfy the struct's field types.
fn build_test_context_model(app: &mut App) -> ModelHandle<BlocklistAIContextModel> {
    initialize_history_persistence_for_tests(app);
    app.add_singleton_model(|_| BlocklistAIHistoryModel::new_for_test());
    let terminal_model = Arc::new(FairMutex::new(TerminalModel::new_for_test(
        block_size(),
        color::List::from(&Colors::default()),
        ChannelEventListener::new_for_test(),
        Arc::new(Background::default()),
        false, /* should_show_bootstrap_block */
        None,  /* restored_blocks */
        false, /* honor_ps1 */
        false, /* is_inverted */
        None,  /* session_startup_path */
    )));
    let terminal_view_id = EntityId::new();

    let conversation_selection = app.add_model(|ctx| {
        Box::new(TestConversationSelection::new(terminal_view_id, ctx))
            as Box<dyn ConversationSelection>
    });

    app.add_model(|_| {
        BlocklistAIContextModel::new_for_test(
            terminal_model,
            terminal_view_id,
            conversation_selection,
        )
    })
}

fn make_image_attachment(file_name: &str) -> PendingAttachment {
    PendingAttachment::Image(ImageContext {
        data: String::new(),
        mime_type: "image/png".to_owned(),
        file_name: file_name.to_owned(),
        is_figma: false,
    })
}

fn make_file_attachment(file_name: &str) -> PendingAttachment {
    PendingAttachment::File(PendingFile {
        file_name: file_name.to_owned(),
        file_path: file_name.into(),
        mime_type: "text/plain".to_owned(),
    })
}

#[test]
fn has_locking_attachment_is_false_for_default_state() {
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.read(&app, |m, _| {
            assert!(!m.has_locking_attachment());
        });
    });
}

#[test]
fn has_locking_attachment_is_true_with_pending_block_id() {
    // Zap keeps attached blocks as locking attachments: an explicitly attached block is a
    // signal that the next query is intended for the agent.
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.insert_pending_block_id_for_test(BlockId::new());
        });

        model.read(&app, |m, _| assert!(m.has_locking_attachment()));
    });
}

#[test]
fn repository_context_from_repository_info_converts_to_agent_context() {
    let repository_info = RepositoryInfo {
        name: "warp-internal".to_owned(),
        owner: Some("warpdotdev".to_owned()),
        host: Some("github.com".to_owned()),
    };

    assert_eq!(
        BlocklistAIContextModel::repository_context_from_repository_info(&repository_info),
        AIAgentContext::Repository {
            name: "warp-internal".to_owned(),
            owner: Some("warpdotdev".to_owned()),
            host: Some("github.com".to_owned()),
        }
    );
}

#[test]
fn pull_request_context_from_pr_info_includes_url() {
    let pr_info = PrInfo {
        number: 123,
        url: "https://github.com/warpdotdev/warp/pull/123".to_owned(),
        state: "OPEN".to_owned(),
        draft: true,
        base_branch: "main".to_owned(),
    };

    assert_eq!(
        BlocklistAIContextModel::pull_request_context_from_pr_info(&pr_info),
        Some(AIAgentContext::PullRequest {
            number: 123,
            state: "OPEN".to_owned(),
            draft: true,
            base_branch: "main".to_owned(),
            url: "https://github.com/warpdotdev/warp/pull/123".to_owned(),
        })
    );
}

#[test]
fn pull_request_context_from_pr_info_rejects_numbers_that_do_not_fit_agent_context() {
    let pr_info = PrInfo {
        number: i32::MAX as u64 + 1,
        url: "https://github.com/warpdotdev/warp/pull/2147483648".to_owned(),
        state: "OPEN".to_owned(),
        draft: false,
        base_branch: "main".to_owned(),
    };

    assert_eq!(
        BlocklistAIContextModel::pull_request_context_from_pr_info(&pr_info),
        None
    );
}

#[cfg(feature = "local_fs")]
#[test]
fn pull_request_context_reads_github_repo_model() {
    App::test((), |mut app| async move {
        let context_model = build_test_context_model(&mut app);
        let temp_dir = tempfile::TempDir::new().unwrap();
        let watcher_handle = app.add_singleton_model(DirectoryWatcher::new_for_testing);
        let repository = watcher_handle.update(&mut app, |watcher, ctx| {
            watcher
                .add_directory(
                    StandardizedPath::from_local_canonicalized(temp_dir.path()).unwrap(),
                    ctx,
                )
                .unwrap()
        });
        let git_status =
            app.add_model(move |ctx| GitRepoStatusModel::new_local_for_test(repository, None, ctx));
        let github_repo_model =
            app.add_model(move |ctx| GitHubRepoModel::new_local_for_test(git_status, ctx));

        github_repo_model.update(&mut app, |model, ctx| {
            model.set_pr_info_for_test(
                Some(PrInfo {
                    number: 123,
                    url: "https://github.com/warpdotdev/warp/pull/123".to_owned(),
                    state: "OPEN".to_owned(),
                    draft: false,
                    base_branch: "main".to_owned(),
                }),
                ctx,
            );
        });

        context_model.update(&mut app, |model, _| {
            model.set_github_repo_model(Some(github_repo_model.downgrade()));
        });

        context_model.read(&app, |model, ctx| {
            assert_eq!(
                model.pull_request_context(ctx),
                Some(AIAgentContext::PullRequest {
                    number: 123,
                    state: "OPEN".to_owned(),
                    draft: false,
                    base_branch: "main".to_owned(),
                    url: "https://github.com/warpdotdev/warp/pull/123".to_owned(),
                })
            );
        });

        context_model.update(&mut app, |model, _| {
            model.set_github_repo_model(None);
        });

        context_model.read(&app, |model, ctx| {
            assert_eq!(model.pull_request_context(ctx), None);
        });
    });
}

#[test]
fn has_locking_attachment_is_false_with_only_pending_selected_text() {
    // Selected text alone is *not* a locking attachment: the user could be selecting shell
    // command text (e.g. to copy a previously-run command), and forcing the input into AI
    // mode in that case would be wrong. Only images, files, or blocks should force the lock.
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.set_pending_selected_text_for_test(Some("hello".to_owned()));
        });

        model.read(&app, |m, _| assert!(!m.has_locking_attachment()));
    });
}

#[test]
fn has_locking_attachment_is_true_with_pending_image_attachment() {
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.append_pending_attachments_for_test(vec![make_image_attachment("a.png")]);
        });

        model.read(&app, |m, _| assert!(m.has_locking_attachment()));
    });
}

#[test]
fn has_locking_attachment_is_true_with_only_file_attachments() {
    // File attachments are locking attachments — the user has explicitly attached a file as
    // context, which is unambiguously a signal that the next query is intended for the agent.
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.append_pending_attachments_for_test(vec![
                make_file_attachment("notes.txt"),
                make_file_attachment("readme.md"),
            ]);
        });

        model.read(&app, |m, _| assert!(m.has_locking_attachment()));
    });
}

#[test]
fn has_locking_attachment_is_true_with_mixed_image_and_file_attachments() {
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.append_pending_attachments_for_test(vec![
                make_file_attachment("notes.txt"),
                make_image_attachment("a.png"),
            ]);
        });

        model.read(&app, |m, _| assert!(m.has_locking_attachment()));
    });
}

#[test]
fn take_pending_attachments_drains_and_returns_all_staged() {
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);
        model.update(&mut app, |m, _| {
            m.append_pending_attachments_for_test(vec![
                make_image_attachment("a.png"),
                make_file_attachment("notes.txt"),
            ]);
        });

        let taken = model.update(&mut app, |m, ctx| m.take_pending_attachments(ctx));
        assert_eq!(taken.len(), 2);
        assert_eq!(taken[0].file_name(), "a.png");
        assert_eq!(taken[1].file_name(), "notes.txt");

        // Draining clears the live staging so the input's attachment chips disappear.
        model.read(&app, |m, _| assert!(m.pending_attachments().is_empty()));
    });
}

#[test]
fn enqueue_moves_staged_attachments_onto_the_row_and_clears_input() {
    // Mirrors the enqueue sites in `input.rs`: `take_pending_attachments` drains the live input
    // staging and the drained set is stored on the queued row via `new_with_attachments`, leaving
    // no attachments behind in the input.
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);
        let queued = app.add_singleton_model(QueuedQueryModel::new);
        let conv = AIConversationId::new();

        model.update(&mut app, |m, _| {
            m.append_pending_attachments_for_test(vec![
                make_image_attachment("a.png"),
                make_file_attachment("notes.txt"),
            ]);
        });

        // Capture-and-clear, then store on the row (the exact composition used at enqueue time).
        let taken = model.update(&mut app, |m, ctx| m.take_pending_attachments(ctx));
        let id = queued.update(&mut app, |q, ctx| {
            q.append(
                conv,
                QueuedQuery::new_with_attachments(
                    "queued".to_owned(),
                    QueuedQueryOrigin::AutoQueueToggle,
                    taken,
                ),
                ctx,
            )
        });

        // Live staging is cleared; the row owns the attachments.
        model.read(&app, |m, _| assert!(m.pending_attachments().is_empty()));
        queued.read(&app, |q, _| {
            let attachments = q.attachments_for(conv, id);
            assert_eq!(attachments.len(), 2);
            assert_eq!(attachments[0].file_name(), "a.png");
            assert_eq!(attachments[1].file_name(), "notes.txt");
        });
    });
}

#[test]
fn referenced_at_context_attachments_prefers_longest_visible_reference() {
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.register_at_context_attachment(
                "@commit".to_owned(),
                AIAgentAttachment::PlainText("old".to_owned()),
            );
            m.register_at_context_attachment(
                "@commit (4)".to_owned(),
                AIAgentAttachment::PlainText("new".to_owned()),
            );
        });

        model.read(&app, |m, _| {
            let attachments = m.referenced_at_context_attachments("@commit (4) hi");
            assert_eq!(attachments.len(), 1);
            assert_eq!(
                attachments.get("@commit (4)"),
                Some(&AIAgentAttachment::PlainText("new".to_owned()))
            );
        });
    });
}

#[test]
fn retain_at_context_attachments_in_query_drops_deleted_prefix_reference() {
    App::test((), |mut app| async move {
        let model = build_test_context_model(&mut app);

        model.update(&mut app, |m, _| {
            m.register_at_context_attachment(
                "@commit".to_owned(),
                AIAgentAttachment::PlainText("old".to_owned()),
            );
            m.register_at_context_attachment(
                "@commit (4)".to_owned(),
                AIAgentAttachment::PlainText("new".to_owned()),
            );
            m.retain_at_context_attachments_in_query("@commit (4) hi");
        });

        model.read(&app, |m, _| {
            assert!(!m.pending_at_context_attachments().contains_key("@commit"));
            assert!(
                m.pending_at_context_attachments()
                    .contains_key("@commit (4)")
            );
        });
    });
}

#[cfg(feature = "local_fs")]
fn project_rule_contents(context: &[AIAgentContext]) -> Vec<String> {
    context
        .iter()
        .filter_map(|context| match context {
            AIAgentContext::ProjectRules { active_rules, .. } => Some(active_rules),
            _ => None,
        })
        .flatten()
        .map(|rule| match &rule.content {
            AnyFileContent::StringContent(content) => content.clone(),
            other => panic!("unexpected rule content: {other:?}"),
        })
        .collect()
}

#[cfg(feature = "local_fs")]
#[test]
fn pending_context_resolves_remote_cwd_rules_on_the_remote_host() {
    App::test((), |mut app| async move {
        let context_model = build_test_context_model(&mut app);
        let project_context = app.add_singleton_model(|_| ProjectContextModel::default());

        // The remote cwd also exists locally with its own AGENTS.md, as when both machines share a
        // username. A remote session must get the remote host's rules, never this local file.
        let temp_dir = tempfile::TempDir::new().unwrap();
        let cwd = temp_dir.path().canonicalize().unwrap();
        std::fs::write(cwd.join("AGENTS.md"), "local rules").unwrap();

        let host_id = HostId::new("remote-host".to_owned());
        project_context.update(&mut app, |model, _| {
            model.set_remote_global_rules(
                host_id.clone(),
                vec![ProjectRule {
                    path: LocalOrRemotePath::Remote(RemotePath::new(
                        host_id.clone(),
                        StandardizedPath::try_new("/home/me/.agents/AGENTS.md").unwrap(),
                    )),
                    content: "remote rules".to_owned(),
                }],
            );
        });

        let remote_cwd = LocalOrRemotePath::Remote(RemotePath::new(
            host_id,
            StandardizedPath::from_local_canonicalized(&cwd).unwrap(),
        ));
        let local_cwd = LocalOrRemotePath::Local(cwd);

        context_model.read(&app, |model, ctx| {
            assert_eq!(
                project_rule_contents(&model.pending_context(ctx, false, Some(&remote_cwd))),
                vec!["remote rules".to_owned()]
            );
            assert_eq!(
                project_rule_contents(&model.pending_context(ctx, false, Some(&local_cwd))),
                vec!["local rules".to_owned()]
            );
            assert!(project_rule_contents(&model.pending_context(ctx, false, None)).is_empty());
        });
    });
}
