use std::fs;
use std::path::Path;

use repo_metadata::{DirectoryWatcher, TargetFile};
use warp_util::standardized_path::StandardizedPath;
use warpui::{App, ReadModel};

use super::*;
use crate::test_util::assert_eventually;
use crate::test_util::settings::initialize_settings_for_tests;

fn initialize_outline_test(app: &mut App) -> ModelHandle<RepoOutlines> {
    initialize_settings_for_tests(app);
    // The disconnected new_for_testing stub cannot acknowledge filesystem watch registration.
    app.add_singleton_model(DirectoryWatcher::new);
    app.add_singleton_model(|_| DetectedRepositories::default());
    app.add_singleton_model(RepoOutlines::new_for_test)
}

fn add_test_repository(app: &mut App, root: &Path) -> ModelHandle<Repository> {
    DirectoryWatcher::handle(app)
        .update(app, |watcher, ctx| {
            watcher.add_directory(
                StandardizedPath::from_local_canonicalized(root).unwrap(),
                ctx,
            )
        })
        .unwrap()
}

fn symbol_names(outlines: &RepoOutlines, root: &Path) -> Vec<String> {
    let Some(OutlineStatus::Complete(outline)) = outlines.get_outline_for_repo(root) else {
        panic!("expected a completed local outline");
    };
    outline
        .to_symbols_by_file(None)
        .into_values()
        .flat_map(|file| {
            file.symbols()
                .into_iter()
                .flatten()
                .map(|symbol| symbol.name.clone())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[test]
fn local_outline_builds_updates_and_respects_ignored_files() {
    App::test((), |mut app| async move {
        let _flag = FeatureFlag::AIContextMenuCode.override_enabled(true);
        initialize_settings_for_tests(&mut app);
        let watcher = app.add_singleton_model(DirectoryWatcher::new);
        app.add_singleton_model(|_| DetectedRepositories::default());
        let outlines = app.add_singleton_model(RepoOutlines::new_for_test);
        let temp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        fs::create_dir(root.join("ignored")).unwrap();
        fs::write(root.join(".gitignore"), "ignored/\n").unwrap();
        fs::write(root.join("main.rs"), "pub fn public_symbol() {}\n").unwrap();
        fs::write(root.join("ignored/secret.rs"), "fn ignored_symbol() {}\n").unwrap();
        let repository = watcher
            .update(&mut app, |watcher, ctx| {
                watcher.add_directory(
                    StandardizedPath::from_local_canonicalized(&root).unwrap(),
                    ctx,
                )
            })
            .unwrap();

        outlines.update(&mut app, |outlines, ctx| {
            assert!(*InputSettings::as_ref(ctx).outline_codebase_symbols_for_at_context_menu);
            outlines.index_repo(repository, ctx);
            assert!(matches!(
                outlines.get_outline(&root),
                Some((OutlineStatus::Pending, _))
            ));
        });
        assert_eventually!(
            1000 => outlines.read(&app, |outlines, _| outlines.is_directory_indexed(&root)),
            "local tree-sitter outline should complete"
        );
        let names = outlines.read(&app, |outlines, _| symbol_names(outlines, &root));
        assert!(names.iter().any(|name| name == "public_symbol"));
        assert!(!names.iter().any(|name| name == "ignored_symbol"));
        assert!(outlines.read(&app, |outlines, _| {
            outlines.is_directory_indexed(&root.join("main.rs"))
        }));

        fs::write(root.join("main.rs"), "pub fn updated_symbol() {}\n").unwrap();
        outlines.update(&mut app, |outlines, ctx| {
            outlines.handle_repository_update(
                &root,
                RepositoryUpdate {
                    modified: HashSet::from([TargetFile::new(root.join("main.rs"), false)]),
                    ..Default::default()
                },
                ctx,
            );
        });
        assert_eventually!(
            1000 => outlines.read(&app, |outlines, _| outlines.is_directory_indexed(&root)),
            "updated tree-sitter outline should complete"
        );
        let names = outlines.read(&app, |outlines, _| symbol_names(outlines, &root));
        assert!(names.iter().any(|name| name == "updated_symbol"));
        assert!(!names.iter().any(|name| name == "public_symbol"));

        InputSettings::handle(&app).update(&mut app, |settings, ctx| {
            settings
                .outline_codebase_symbols_for_at_context_menu
                .set_value(false, ctx)
                .unwrap();
        });
        outlines.read(&app, |outlines, _| {
            assert!(outlines.outlines.is_empty());
            assert!(outlines.active_outline_task.is_none());
        });
        assert!(!outlines.read(&app, |outlines, _| outlines.is_directory_indexed(&root)));
    });
}

#[test]
fn nested_detected_repositories_have_distinct_outline_roots() {
    App::test((), |mut app| async move {
        let _flag = FeatureFlag::AIContextMenuCode.override_enabled(true);
        let outlines = initialize_outline_test(&mut app);
        let temp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        let nested = root.join("nested");
        fs::create_dir(root.join(".git")).unwrap();
        fs::create_dir_all(nested.join(".git")).unwrap();
        fs::write(root.join("parent.rs"), "fn parent_symbol() {}\n").unwrap();
        fs::write(nested.join("child.rs"), "fn child_symbol() {}\n").unwrap();
        let parent_repository = add_test_repository(&mut app, &root);
        let nested_repository = add_test_repository(&mut app, &nested);

        outlines.update(&mut app, |outlines, ctx| {
            outlines.index_repo(parent_repository.clone(), ctx);
            assert!(outlines.get_outline_for_repo(&nested).is_none());
            outlines.index_repo(parent_repository, ctx);
            outlines.index_repo(nested_repository, ctx);
            assert_eq!(outlines.outlines.len(), 2);
            assert!(matches!(
                outlines.get_outline_for_repo(&nested),
                Some(OutlineStatus::Pending)
            ));
        });
        assert_eventually!(
            1000 => outlines.read(&app, |outlines, _| {
                matches!(outlines.get_outline_for_repo(&root), Some(OutlineStatus::Complete(_)))
                    && matches!(outlines.get_outline_for_repo(&nested), Some(OutlineStatus::Complete(_)))
            }),
            "parent and nested repositories should both finish outlining"
        );
        let names = outlines.read(&app, |outlines, _| symbol_names(outlines, &nested));
        assert!(names.iter().any(|name| name == "child_symbol"));
        assert!(!names.iter().any(|name| name == "parent_symbol"));
    });
}

#[test]
fn failed_watch_registration_does_not_start_outline_parse() {
    App::test((), |mut app| async move {
        let _flag = FeatureFlag::AIContextMenuCode.override_enabled(true);
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(DirectoryWatcher::new_for_testing);
        app.add_singleton_model(|_| DetectedRepositories::default());
        let outlines = app.add_singleton_model(RepoOutlines::new_for_test);
        let temp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        fs::write(root.join("main.rs"), "fn initial_symbol() {}\n").unwrap();
        let repository = add_test_repository(&mut app, &root);
        outlines.update(&mut app, |outlines, ctx| {
            outlines.index_repo(repository, ctx)
        });

        assert_eventually!(
            1000 => outlines.read(&app, |outlines, _| {
                matches!(outlines.get_outline_for_repo(&root), Some(OutlineStatus::Failed))
            }),
            "watcher registration failure should fail the outline without starting parsing"
        );
        outlines.read(&app, |outlines, _| {
            assert!(outlines.active_outline_task.is_none());
            assert!(outlines.outline_queue.is_empty());
            assert!(outlines.outlines[&root].subscriber_id.is_none());
            assert!(!outlines.is_directory_indexed(&root));
        });
    });
}

#[test]
fn outline_registers_watcher_before_starting_initial_parse() {
    App::test((), |mut app| async move {
        let _flag = FeatureFlag::AIContextMenuCode.override_enabled(true);
        let outlines = initialize_outline_test(&mut app);
        let temp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        fs::write(root.join("main.rs"), "fn initial_symbol() {}\n").unwrap();
        let repository = add_test_repository(&mut app, &root);
        outlines.update(&mut app, |outlines, ctx| {
            outlines.index_repo(repository, ctx);
            assert!(outlines.outlines[&root].subscriber_id.is_some());
            assert!(outlines.active_outline_task.is_none());
            assert!(outlines.outline_queue.is_empty());
            outlines.handle_repository_update(
                &root,
                RepositoryUpdate {
                    modified: HashSet::from([TargetFile::new(root.join("main.rs"), false)]),
                    ..Default::default()
                },
                ctx,
            );
            assert_eq!(outlines.outlines[&root].pending_updates.len(), 1);
        });
        assert_eventually!(
            1000 => outlines.read(&app, |outlines, _| outlines.is_directory_indexed(&root)),
            "registered outline should build and drain early updates"
        );
    });
}

#[test]
fn updates_during_initial_parse_are_replayed_before_completion() {
    App::test((), |mut app| async move {
        let _flag = FeatureFlag::AIContextMenuCode.override_enabled(true);
        let outlines = initialize_outline_test(&mut app);
        let temp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        fs::write(root.join("main.rs"), "fn old_symbol() {}\n").unwrap();
        let repository = add_test_repository(&mut app, &root);
        let initial_snapshot = build_outline(&root, Some(MAX_REPO_FILE_SIZE_LIMIT))
            .await
            .unwrap();
        fs::write(root.join("main.rs"), "fn current_symbol() {}\n").unwrap();
        outlines.update(&mut app, |outlines, ctx| {
            outlines.outlines.insert(
                root.clone(),
                OutlineState {
                    repository,
                    status: OutlineStatus::Pending,
                    subscriber_id: None,
                    pending_updates: Vec::new(),
                },
            );
            outlines.handle_repository_update(
                &root,
                RepositoryUpdate {
                    modified: HashSet::from([TargetFile::new(root.join("main.rs"), false)]),
                    ..Default::default()
                },
                ctx,
            );
            outlines.finish_outline(root.clone(), initial_snapshot, ctx);
            assert!(matches!(
                outlines.get_outline_for_repo(&root),
                Some(OutlineStatus::Pending)
            ));
        });
        assert_eventually!(
            1000 => outlines.read(&app, |outlines, _| outlines.is_directory_indexed(&root)),
            "initial snapshot must replay updates received during parsing"
        );
        let names = outlines.read(&app, |outlines, _| symbol_names(outlines, &root));
        assert!(names.iter().any(|name| name == "current_symbol"));
        assert!(!names.iter().any(|name| name == "old_symbol"));
    });
}

#[test]
fn updates_during_incremental_parse_are_drained_in_order() {
    App::test((), |mut app| async move {
        let _flag = FeatureFlag::AIContextMenuCode.override_enabled(true);
        let outlines = initialize_outline_test(&mut app);
        let temp = tempfile::tempdir().unwrap();
        let root = dunce::canonicalize(temp.path()).unwrap();
        fs::write(root.join("main.rs"), "fn old_symbol() {}\n").unwrap();
        let repository = add_test_repository(&mut app, &root);
        let initial_snapshot = build_outline(&root, Some(MAX_REPO_FILE_SIZE_LIMIT))
            .await
            .unwrap();
        fs::write(root.join("main.rs"), "fn updated_symbol() {}\n").unwrap();
        fs::write(root.join("added.rs"), "fn added_symbol() {}\n").unwrap();
        outlines.update(&mut app, |outlines, ctx| {
            outlines.outlines.insert(
                root.clone(),
                OutlineState {
                    repository,
                    status: OutlineStatus::Complete(initial_snapshot),
                    subscriber_id: None,
                    pending_updates: Vec::new(),
                },
            );
            outlines.handle_repository_update(
                &root,
                RepositoryUpdate {
                    modified: HashSet::from([TargetFile::new(root.join("main.rs"), false)]),
                    ..Default::default()
                },
                ctx,
            );
            outlines.handle_repository_update(
                &root,
                RepositoryUpdate {
                    added: HashSet::from([TargetFile::new(root.join("added.rs"), false)]),
                    ..Default::default()
                },
                ctx,
            );
            assert_eq!(outlines.outlines[&root].pending_updates.len(), 1);
        });
        assert_eventually!(
            1000 => outlines.read(&app, |outlines, _| outlines.is_directory_indexed(&root)),
            "incremental parsing must drain later update batches"
        );
        let names = outlines.read(&app, |outlines, _| symbol_names(outlines, &root));
        assert!(names.iter().any(|name| name == "updated_symbol"));
        assert!(names.iter().any(|name| name == "added_symbol"));
        assert!(!names.iter().any(|name| name == "old_symbol"));
        outlines.read(&app, |outlines, _| {
            assert!(outlines.outlines[&root].pending_updates.is_empty())
        });
    });
}

#[test]
fn local_outline_gate_matches_symbol_menu_flag_and_setting() {
    App::test((), |mut app| async move {
        let flag = FeatureFlag::AIContextMenuCode.override_enabled(true);
        initialize_settings_for_tests(&mut app);
        app.add_singleton_model(|_| DetectedRepositories::default());
        let outlines = app.add_singleton_model(RepoOutlines::new_for_test);
        outlines.update(&mut app, |outlines, ctx| {
            assert!(outlines.should_build_outlines(ctx))
        });
        let disabled = app.add_model(|ctx| RepoOutlines::new_with_indexing_enabled(false, ctx));
        disabled.update(&mut app, |outlines, ctx| {
            assert!(!outlines.should_build_outlines(ctx))
        });
        drop(flag);
        let _flag = FeatureFlag::AIContextMenuCode.override_enabled(false);
        outlines.update(&mut app, |outlines, ctx| {
            assert!(!outlines.should_build_outlines(ctx))
        });
    });
}
