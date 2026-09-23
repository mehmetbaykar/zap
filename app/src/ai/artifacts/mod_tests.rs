use super::*;

#[test]
fn test_parse_github_pr_url() {
    assert_eq!(
        parse_github_pr_url("https://github.com/owner/repo/pull/123"),
        Some(("repo".to_string(), 123))
    );
    assert_eq!(
        parse_github_pr_url("https://github.com/my-org/my-repo/pull/456"),
        Some(("my-repo".to_string(), 456))
    );
    assert_eq!(
        parse_github_pr_url("https://github.com/my-org/my-repo"),
        None
    );
    assert_eq!(parse_github_pr_url("not a url"), None);
}

#[test]
fn artifact_round_trips_all_variants() {
    let artifacts = vec![
        Artifact::Plan {
            document_uid: "doc-1".to_string(),
            notebook_uid: Some(NotebookId::from("0123456789abcdefABCDEF".to_string())),
            title: Some("My plan".to_string()),
        },
        Artifact::Plan {
            document_uid: "doc-2".to_string(),
            notebook_uid: None,
            title: None,
        },
        Artifact::PullRequest {
            url: "https://github.com/warpdotdev/warp/pull/123".to_string(),
            branch: "feature-branch".to_string(),
            repo: Some("warp".to_string()),
            number: Some(123),
        },
        Artifact::Screenshot {
            artifact_uid: "shot-1".to_string(),
            mime_type: "image/png".to_string(),
            description: Some("A screenshot".to_string()),
        },
        Artifact::File {
            artifact_uid: "file-1".to_string(),
            filepath: "outputs/report.txt".to_string(),
            filename: "report.txt".to_string(),
            mime_type: "text/plain".to_string(),
            description: Some("Daily summary".to_string()),
            size_bytes: Some(42),
        },
    ];
    for artifact in artifacts {
        let json = serde_json::to_value(&artifact).unwrap();
        let deserialized: Artifact = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized, artifact);
    }
}

#[test]
fn artifact_pull_request_derives_repo_and_number_on_deserialize() {
    let artifact: Artifact = serde_json::from_str(
        r#"{"artifact_type":"PULL_REQUEST","data":{"url":"https://github.com/warpdotdev/warp/pull/456","branch":"main"}}"#,
    )
    .unwrap();
    assert_eq!(
        artifact,
        Artifact::PullRequest {
            url: "https://github.com/warpdotdev/warp/pull/456".to_string(),
            branch: "main".to_string(),
            repo: Some("warp".to_string()),
            number: Some(456),
        }
    );
}

#[test]
fn artifact_pull_request_without_github_url_has_no_repo_or_number() {
    let artifact: Artifact = serde_json::from_str(
        r#"{"artifact_type":"PULL_REQUEST","data":{"url":"https://example.com/pr/1","branch":"main"}}"#,
    )
    .unwrap();
    assert_eq!(
        artifact,
        Artifact::PullRequest {
            url: "https://example.com/pr/1".to_string(),
            branch: "main".to_string(),
            repo: None,
            number: None,
        }
    );
}

#[test]
fn artifact_rejects_unknown_artifact_type() {
    let result = serde_json::from_str::<Artifact>(r#"{"artifact_type":"UNKNOWN","data":{}}"#);
    assert!(result.is_err());
}
