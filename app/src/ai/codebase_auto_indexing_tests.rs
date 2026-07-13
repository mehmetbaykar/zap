use super::*;

#[test]
fn local_auto_indexing_requires_full_source_code_embedding_codebase_context_and_auto_indexing() {
    {
        let _flag = FeatureFlag::FullSourceCodeEmbedding.override_enabled(false);
        assert!(!codebase_auto_indexing_enabled(
            CodebaseAutoIndexingSurface::Local,
            true,
            true,
        ));
    }
    {
        let _flag = FeatureFlag::FullSourceCodeEmbedding.override_enabled(true);
        assert!(codebase_auto_indexing_enabled(
            CodebaseAutoIndexingSurface::Local,
            true,
            true,
        ));
        assert!(!codebase_auto_indexing_enabled(
            CodebaseAutoIndexingSurface::Local,
            false,
            true,
        ));
        assert!(!codebase_auto_indexing_enabled(
            CodebaseAutoIndexingSurface::Local,
            true,
            false,
        ));
        assert!(!codebase_auto_indexing_enabled(
            CodebaseAutoIndexingSurface::Local,
            false,
            false,
        ));
    }
}

#[test]
fn remote_auto_indexing_stays_disabled_without_the_hosted_backend() {
    let _flag = FeatureFlag::FullSourceCodeEmbedding.override_enabled(true);
    assert!(!codebase_auto_indexing_enabled(
        CodebaseAutoIndexingSurface::Remote,
        true,
        true,
    ));
}

#[test]
fn auto_index_requests_remain_disabled_without_a_local_query_backend() {
    let _embedding_flag = FeatureFlag::FullSourceCodeEmbedding.override_enabled(true);

    assert!(!should_auto_index_codebase(
        CodebaseAutoIndexingSurface::Local
    ));
    assert!(!should_auto_index_codebase(
        CodebaseAutoIndexingSurface::Remote
    ));
}

#[test]
fn candidate_roots_are_deduped_before_filtering() {
    let roots = vec!["/repo", "/repo", "/other"];
    let candidates = auto_index_candidate_roots(roots, |root| *root != "/other");

    assert_eq!(candidates, vec!["/repo"]);
}
