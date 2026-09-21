use warpui::{App, ReadModel};

use super::*;
use crate::search::item::SearchItem;
use crate::test_util::settings::initialize_settings_for_tests;

fn symbol(name: &str, type_prefix: Option<&str>, path: &str, line: usize) -> CodeSymbol {
    CodeSymbol {
        file_path: PathBuf::from(path),
        symbol: Symbol {
            name: name.to_owned(),
            type_prefix: type_prefix.map(str::to_owned),
            comment: None,
            line_number: line,
        },
    }
}

fn item(name: &str, path: &str, query: &str) -> CodeSearchItem {
    let code_symbol = symbol(name, Some("fn"), path, 7);
    let match_result = fuzzy_match_symbol_with_type(&code_symbol, query);
    CodeSearchItem {
        code_symbol,
        match_result,
    }
}

#[test]
fn git_status_snapshot_preserves_changed_file_prioritization() {
    let changed_files = parse_git_changed_files(
        " M src/edited.rs\nA  src/added.rs\n?? new.rs\n M \"with spaces.rs\"\n M src/edited.rs\n",
    );
    assert_eq!(
        changed_files,
        HashSet::from([
            "src/edited.rs".to_owned(),
            "src/added.rs".to_owned(),
            "new.rs".to_owned(),
            "with spaces.rs".to_owned(),
        ])
    );
    let results = finalize_zero_state(
        vec![
            item("other", "other.rs", ""),
            item("changed", "src/edited.rs", ""),
        ],
        &changed_files,
    );
    assert!(results[0].score() > results[1].score());
    assert_eq!(
        results[0].accept_result(),
        AIContextMenuSearchableAction::InsertText {
            text: "fn changed in src/edited.rs:7".to_owned(),
        }
    );
    assert!(parse_git_changed_files("").is_empty());
}

#[test]
fn local_symbol_search_matches_type_name_and_case() {
    let code_symbol = symbol("my_function", Some("fn"), "src/main.rs", 7);
    for query in ["function", "fn", "fn my_function", "MY_FUNCTION"] {
        let result = fuzzy_match_symbol_with_type(&code_symbol, query);
        assert!(result.score > 0, "expected symbol match for {query}");
        assert_eq!(result.score % 3, 0);
    }
    assert_eq!(
        fuzzy_match_symbol_with_type(&code_symbol, "unrelated").score,
        0
    );
    assert_eq!(fuzzy_match_symbol_with_type(&code_symbol, "").score, 0);
    assert!(
        fuzzy_match_symbol_with_type(&symbol("variable", None, "lib.rs", 1), "variable").score > 0
    );
}

#[test]
fn local_symbol_result_preserves_path_line_and_unicode_name() {
    let item = CodeSearchItem {
        code_symbol: symbol("café", Some("fn"), "src/with spaces.rs", 42),
        match_result: FuzzyMatchResult::no_match(),
    };
    let expected = AIContextMenuSearchableAction::InsertText {
        text: "fn café in src/with spaces.rs:42".to_owned(),
    };
    assert_eq!(item.accept_result(), expected);
    assert_eq!(item.execute_result(), expected);
    assert_eq!(
        item.accessibility_label(),
        "Code symbol: café in src/with spaces.rs:42"
    );
}

#[test]
fn local_symbol_query_prioritizes_matching_symbol() {
    let results = finalize_query(vec![
        item("unrelated", "other.rs", "function"),
        item("my_function", "main.rs", "function"),
    ]);
    let best = results.iter().max_by_key(|result| result.score()).unwrap();
    assert_eq!(
        best.accept_result(),
        AIContextMenuSearchableAction::InsertText {
            text: "fn my_function in main.rs:7".to_owned(),
        }
    );
}

#[test]
fn local_symbol_zero_state_prioritizes_changed_files_and_stays_bounded() {
    let results = finalize_zero_state(
        vec![
            item("other", "other.rs", ""),
            item("changed", "changed.rs", ""),
        ],
        &HashSet::from(["changed.rs".to_owned()]),
    );
    assert!(results[0].score() > results[1].score());
    assert_eq!(
        results[0].accept_result(),
        AIContextMenuSearchableAction::InsertText {
            text: "fn changed in changed.rs:7".to_owned(),
        }
    );

    for changed_files in [HashSet::new(), HashSet::from(["main.rs".to_owned()])] {
        let items = (0..MAX_RESULTS + 20)
            .map(|i| item(&format!("symbol_{i}"), "main.rs", ""))
            .collect();
        assert_eq!(
            finalize_zero_state(items, &changed_files).len(),
            MAX_RESULTS
        );
    }
}

#[test]
fn unmatched_symbol_queries_are_empty_without_removing_zero_state_results() {
    assert!(finalize_query(vec![item("unrelated", "other.rs", "zzzzz")]).is_empty());
    let results = finalize_query(vec![
        item("unrelated", "other.rs", "needle"),
        item("needle", "match.rs", "needle"),
    ]);
    assert_eq!(results.len(), 1);
    assert_eq!(
        results[0].accept_result(),
        AIContextMenuSearchableAction::InsertText {
            text: "fn needle in match.rs:7".to_owned(),
        }
    );
    assert_eq!(
        finalize_zero_state(vec![item("unrelated", "other.rs", "")], &HashSet::new()).len(),
        1
    );
}

#[test]
fn valid_nonpositive_symbol_matches_survive_query_finalization() {
    let name = format!("a{}z", "x".repeat(80));
    let code_symbol = symbol(&name, None, "long.rs", 7);
    let match_result = fuzzy_match_symbol_with_type(&code_symbol, "az");
    assert_eq!(match_result.matched_indices, vec![0, 81]);
    assert!(
        match_result.score < 0,
        "long-gap fixture must exercise a negative score"
    );

    for score in [match_result.score, 0] {
        let mut result = match_result.clone();
        result.score = score;
        let results = finalize_query(vec![
            item("unrelated", "other.rs", "az"),
            CodeSearchItem {
                code_symbol: code_symbol.clone(),
                match_result: result,
            },
        ]);
        assert_eq!(results.len(), 1);
        assert_eq!(
            results[0].accept_result(),
            AIContextMenuSearchableAction::InsertText {
                text: format!("{name} in long.rs:7"),
            }
        );
    }
}

#[test]
fn local_symbol_query_is_bounded() {
    let items = (0..MAX_RESULTS + 20)
        .map(|i| item(&format!("symbol_{i}"), "main.rs", "symbol"))
        .collect();
    assert_eq!(finalize_query(items).len(), MAX_RESULTS);
}

#[test]
fn outline_update_invalidates_only_affected_symbol_cache() {
    App::test((), |mut app| async move {
        initialize_settings_for_tests(&mut app);
        let outlines = app.add_singleton_model(RepoOutlines::new_for_test);
        let cache = app.add_model(CodeSymbolCache::new);
        let repo = PathBuf::from("synthetic-repo");
        let other_repo = PathBuf::from("other-synthetic-repo");
        cache.update(&mut app, |cache, _| {
            for path in [&repo, &other_repo] {
                cache.symbol_cache.get_mut().insert(
                    path.clone(),
                    SymbolCache::new(vec![symbol("cached", Some("fn"), "main.rs", 1)]),
                );
            }
        });
        outlines.update(&mut app, |_, ctx| {
            ctx.emit(RepoOutlinesEvent::OutlinesUpdated(repo.clone()));
        });
        cache.read(&app, |cache, _| {
            assert!(!cache.symbol_cache.borrow().contains_key(&repo));
            assert!(cache.symbol_cache.borrow().contains_key(&other_repo));
        });
        cache.update(&mut app, |cache, _| {
            let (cursor, results) =
                cache.search_symbols_chunk(&repo, 0, "cached", Duration::from_millis(5));
            assert_eq!(cursor, usize::MAX);
            assert!(results.is_empty());
        });
    });
}
