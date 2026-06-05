use super::*;

#[test]
fn brightline_triage_scope_accepts_only_ignore_file() {
    let changed = vec![".brightline-ignore".to_string()];
    assert!(
        validate_brightline_triage_scope(
            "architecture_brightline",
            &changed,
            "openspec/changes/architecture-brightline-abcd1234/",
        )
        .is_ok()
    );
}

#[test]
fn brightline_triage_scope_accepts_ignore_plus_spec_dir() {
    let changed = vec![
        ".brightline-ignore".to_string(),
        "openspec/changes/architecture-brightline-abcd1234/proposal.md".to_string(),
    ];
    assert!(
        validate_brightline_triage_scope(
            "architecture_brightline",
            &changed,
            "openspec/changes/architecture-brightline-abcd1234/",
        )
        .is_ok()
    );
}

#[test]
fn brightline_triage_scope_rejects_ignore_mixed_with_code() {
    let changed = vec![".brightline-ignore".to_string(), "src/foo.rs".to_string()];
    let err = validate_brightline_triage_scope(
        "architecture_brightline",
        &changed,
        "openspec/changes/architecture-brightline-abcd1234/",
    )
    .expect_err("mixed-scope diff must be rejected");
    assert_eq!(err, vec!["src/foo.rs".to_string()]);
}

#[test]
fn brightline_triage_scope_accepts_pure_fixes_diff_without_ignore_file() {
    // No `.brightline-ignore` write → the LLM took the fix path.
    // That path is unconstrained.
    let changed = vec!["src/foo.rs".to_string(), "src/bar.rs".to_string()];
    assert!(
        validate_brightline_triage_scope(
            "architecture_brightline",
            &changed,
            "openspec/changes/architecture-brightline-abcd1234/",
        )
        .is_ok()
    );
}

#[test]
fn brightline_triage_scope_noop_for_other_audits() {
    // Non-brightline audits are unaffected: a mixed diff is fine.
    let changed = vec![".brightline-ignore".to_string(), "src/foo.rs".to_string()];
    assert!(
        validate_brightline_triage_scope(
            "drift_audit",
            &changed,
            "openspec/changes/drift-audit-abcd1234/",
        )
        .is_ok()
    );
}

#[test]
fn discard_non_spec_writes_spec_only_returns_empty() {
    let (_d, ws) = dnsw_repo();
    std::fs::create_dir_all(ws.join("openspec/changes/foo")).unwrap();
    std::fs::write(ws.join("openspec/changes/foo/proposal.md"), "## Why\nx\n").unwrap();
    let dropped = discard_non_spec_writes(&ws, "foo").unwrap();
    assert!(
        dropped.is_empty(),
        "spec-only diff drops nothing: {dropped:?}"
    );
    assert!(
        ws.join("openspec/changes/foo/proposal.md").exists(),
        "spec file must be left untouched"
    );
}

#[test]
fn discard_non_spec_writes_code_only_restores_and_removes() {
    let (_d, ws) = dnsw_repo();
    // Modify a tracked file AND add an untracked code file.
    std::fs::write(ws.join("src/bar.rs"), "MUTATED\n").unwrap();
    std::fs::write(ws.join("newcode.rs"), "junk\n").unwrap();
    let dropped = discard_non_spec_writes(&ws, "foo").unwrap();
    assert_eq!(
        dropped,
        vec!["newcode.rs".to_string(), "src/bar.rs".to_string()]
    );
    // Tracked modification reverted; untracked addition removed.
    assert_eq!(
        std::fs::read_to_string(ws.join("src/bar.rs")).unwrap(),
        "orig\n"
    );
    assert!(!ws.join("newcode.rs").exists());
    assert_eq!(
        crate::git::status_porcelain(&ws).unwrap(),
        "",
        "working tree must be clean after discarding all code writes"
    );
}

#[test]
fn discard_non_spec_writes_mixed_keeps_spec_drops_code() {
    let (_d, ws) = dnsw_repo();
    std::fs::create_dir_all(ws.join("openspec/changes/foo")).unwrap();
    std::fs::write(ws.join("openspec/changes/foo/proposal.md"), "## Why\nx\n").unwrap();
    std::fs::write(ws.join("src/bar.rs"), "MUTATED\n").unwrap();
    let dropped = discard_non_spec_writes(&ws, "foo").unwrap();
    assert_eq!(dropped, vec!["src/bar.rs".to_string()]);
    assert!(
        ws.join("openspec/changes/foo/proposal.md").exists(),
        "spec file must survive a mixed diff"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("src/bar.rs")).unwrap(),
        "orig\n"
    );
}

#[test]
fn discard_non_spec_writes_untracked_and_modified_mix() {
    let (_d, ws) = dnsw_repo();
    // Untracked spec file (kept) + modified tracked code (restored) +
    // untracked nested code file (removed).
    std::fs::create_dir_all(ws.join("openspec/changes/foo")).unwrap();
    std::fs::write(ws.join("openspec/changes/foo/tasks.md"), "- [ ] x\n").unwrap();
    std::fs::write(ws.join("src/bar.rs"), "MUTATED\n").unwrap();
    std::fs::create_dir_all(ws.join("src/sub")).unwrap();
    std::fs::write(ws.join("src/sub/new.rs"), "n\n").unwrap();
    let dropped = discard_non_spec_writes(&ws, "foo").unwrap();
    assert_eq!(
        dropped,
        vec!["src/bar.rs".to_string(), "src/sub/new.rs".to_string()]
    );
    assert!(ws.join("openspec/changes/foo/tasks.md").exists());
    assert_eq!(
        std::fs::read_to_string(ws.join("src/bar.rs")).unwrap(),
        "orig\n"
    );
    assert!(!ws.join("src/sub/new.rs").exists());
}

#[test]
fn discard_non_spec_writes_clean_tree_noop() {
    let (_d, ws) = dnsw_repo();
    let dropped = discard_non_spec_writes(&ws, "foo").unwrap();
    assert!(dropped.is_empty());
    assert_eq!(crate::git::status_porcelain(&ws).unwrap(), "");
}

/// a43 revision: a code edit the executor *staged* with `git add`
/// must be fully reverted — index AND worktree. A plain
/// `git restore -- <path>` only rewrites the worktree from the index,
/// so the staged modification would survive in the index and leak
/// into the supposedly spec-only commit. Because the path exists in
/// HEAD, the handler reverts it with `git checkout HEAD -- <path>`,
/// which unstages AND reverts regardless of the staged state.
#[test]
fn discard_non_spec_writes_reverts_staged_code_modification() {
    let (_d, ws) = dnsw_repo();
    std::fs::write(ws.join("src/bar.rs"), "STAGED MUTATION\n").unwrap();
    // Stage the code edit the way an LLM bash tool might.
    let st = std::process::Command::new("git")
        .args(["add", "src/bar.rs"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(st.success(), "staging src/bar.rs failed");
    let dropped = discard_non_spec_writes(&ws, "foo").unwrap();
    assert_eq!(dropped, vec!["src/bar.rs".to_string()]);
    // Worktree reverted to the committed base content...
    assert_eq!(
        std::fs::read_to_string(ws.join("src/bar.rs")).unwrap(),
        "orig\n"
    );
    // ...AND nothing staged survives: the index is clean, so the
    // caller's `git add -A` + commit cannot sweep the code edit into
    // the spec-only PR.
    assert_eq!(
        crate::git::status_porcelain(&ws).unwrap(),
        "",
        "a staged code modification must be fully unstaged and reverted"
    );
}

/// a43 revision: a brand-new code file the executor created AND staged
/// with `git add` (porcelain `A `, NOT present in HEAD) must be cleanly
/// discarded — unstaged AND removed from disk — NOT aborted with a
/// pathspec error. `git checkout HEAD -- <path>` / `git restore
/// --source=HEAD` reject a path absent from HEAD on some git versions;
/// the handler routes not-in-HEAD tracked paths through `git reset` +
/// disk removal so the common "LLM `git add`ed a new file" case does
/// not crash the triage flow.
#[test]
fn discard_non_spec_writes_discards_staged_new_file() {
    let (_d, ws) = dnsw_repo();
    std::fs::write(ws.join("newcode.rs"), "junk\n").unwrap();
    // Stage the brand-new file the way an LLM bash tool might.
    let st = std::process::Command::new("git")
        .args(["add", "newcode.rs"])
        .current_dir(&ws)
        .status()
        .unwrap();
    assert!(st.success(), "staging newcode.rs failed");
    // Sanity: it is a STAGED ADD (`A `), not untracked (`??`) — so it
    // takes the tracked, not-in-HEAD branch, the one the old `git
    // restore --source=HEAD` would have choked on.
    let porc = crate::git::status_porcelain(&ws).unwrap();
    assert!(
        porc.starts_with("A "),
        "expected a staged addition (A ), got {porc:?}"
    );

    let dropped = discard_non_spec_writes(&ws, "foo").unwrap();

    assert_eq!(dropped, vec!["newcode.rs".to_string()]);
    assert!(
        !ws.join("newcode.rs").exists(),
        "the staged new file must be removed from disk"
    );
    assert_eq!(
        crate::git::status_porcelain(&ws).unwrap(),
        "",
        "a staged new file must be fully unstaged AND removed — nothing \
         left for the caller's `git add -A` to sweep into the spec PR"
    );
}

/// a43 revision: when an untracked non-spec write cannot be removed
/// (here: a write-protected parent directory blocks the unlink), the
/// helper must fail loudly rather than silently leave the file for
/// the caller's `git add -A` to sweep into the spec-only PR.
#[test]
fn discard_non_spec_writes_errors_when_removal_fails() {
    use std::os::unix::fs::PermissionsExt;
    let (_d, ws) = dnsw_repo();
    // An untracked code file inside a directory we then strip write
    // permission from (`r-xr-xr-x`) so `remove_file` fails with
    // PermissionDenied — the unlink needs write permission on the
    // parent directory, not on the file itself.
    let locked = ws.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    std::fs::write(locked.join("leak.rs"), "junk\n").unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();

    let result = discard_non_spec_writes(&ws, "foo");

    // Restore write permission so the TempDir guard can clean up,
    // regardless of the assertion outcome below.
    let _ = std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755));

    assert!(
        result.is_err(),
        "a removal failure must surface as an error, not a silent leak"
    );
    assert!(
        ws.join("locked/leak.rs").exists(),
        "the un-removable file must still be on disk — the error has to \
         prevent the spec commit rather than the file being quietly gone"
    );
}

/// a43 revision: paths git would quote under the default
/// `core.quotePath` (a space AND non-ASCII bytes both trigger it) must
/// still be parsed AND acted on correctly. `git::status_entries`
/// uses `-z`, which disables quoting; the pre-fix default-format parse
/// would yield the literal `"f\303\266\303\266.rs"`, so `remove_file`
/// would NotFound-no-op AND the real file would survive into the
/// spec-only commit. A quoted SPEC path must likewise keep its
/// `openspec/changes/` prefix so it is NOT misclassified as non-spec.
#[test]
fn discard_non_spec_writes_handles_quoted_special_char_paths() {
    let (_d, ws) = dnsw_repo();
    // Untracked code files whose names force quoting: a space AND
    // non-ASCII. Both must be dropped.
    std::fs::write(ws.join("a b.rs"), "junk\n").unwrap();
    std::fs::write(ws.join("föö.rs"), "junk\n").unwrap();
    // A spec file with a quote-forcing name must be KEPT.
    std::fs::create_dir_all(ws.join("openspec/changes/foo")).unwrap();
    std::fs::write(ws.join("openspec/changes/foo/néw.md"), "## Why\nx\n").unwrap();

    let dropped = discard_non_spec_writes(&ws, "foo").unwrap();

    // Sorted: "a b.rs" (0x61) precedes "föö.rs" (0x66).
    assert_eq!(
        dropped,
        vec!["a b.rs".to_string(), "föö.rs".to_string()],
        "both quote-forcing untracked code paths must be parsed AND dropped"
    );
    assert!(
        !ws.join("a b.rs").exists(),
        "the spaced path must be removed"
    );
    assert!(
        !ws.join("föö.rs").exists(),
        "the non-ASCII path must be removed"
    );
    assert!(
        ws.join("openspec/changes/foo/néw.md").exists(),
        "a quote-forcing spec path must be kept, not discarded"
    );
}

/// a43 revision: a STAGED rename of a tracked code file must be fully
/// undone. Under `-z` the rename record is `dest\0source\0`, so the
/// parser MUST consume both fields (else the source path leaks back as
/// a bogus untracked entry AND the rename half-survives). Both sides
/// revert to the committed state.
#[test]
fn discard_non_spec_writes_reverts_staged_rename() {
    let (_d, ws) = dnsw_repo();
    let run = |args: &[&str]| {
        let st = std::process::Command::new("git")
            .args(args)
            .current_dir(&ws)
            .status()
            .unwrap();
        assert!(st.success(), "git {args:?} failed");
    };
    // Stage a rename the way an LLM bash tool might.
    run(&["mv", "src/bar.rs", "src/renamed.rs"]);
    let dropped = discard_non_spec_writes(&ws, "foo").unwrap();
    assert_eq!(
        dropped,
        vec!["src/bar.rs".to_string(), "src/renamed.rs".to_string()],
        "both the rename destination AND source must be reported AND reverted"
    );
    assert_eq!(
        std::fs::read_to_string(ws.join("src/bar.rs")).unwrap(),
        "orig\n",
        "the rename source must be restored to its committed content"
    );
    assert!(
        !ws.join("src/renamed.rs").exists(),
        "the rename destination must be removed"
    );
    assert_eq!(
        crate::git::status_porcelain(&ws).unwrap(),
        "",
        "a staged rename must be fully undone — index AND worktree"
    );
}

/// a43 revision: an untracked SYMLINK must be unlinked (dropping just
/// the link), NOT followed. `is_dir()` follows the link, so a
/// symlink-to-directory would route into `remove_dir_all` and could
/// wipe the TARGET's contents; `symlink_metadata` routes it to
/// `remove_file` instead. The link target must be left intact.
#[test]
fn discard_non_spec_writes_unlinks_symlink_without_following() {
    use std::os::unix::fs::symlink;
    let (_d, ws) = dnsw_repo();
    // A directory OUTSIDE the repo (its own temp dir, so git status
    // never sees it) holding a file we must not touch.
    let target_guard = tempfile::TempDir::new().unwrap();
    let target = target_guard.path().join("outside-target");
    std::fs::create_dir_all(&target).unwrap();
    std::fs::write(target.join("precious.txt"), "do not delete\n").unwrap();
    // An untracked symlink inside the repo pointing at that directory.
    symlink(&target, ws.join("linkdir")).unwrap();

    let dropped = discard_non_spec_writes(&ws, "foo").unwrap();

    assert_eq!(dropped, vec!["linkdir".to_string()]);
    assert!(
        !ws.join("linkdir").exists(),
        "the untracked symlink must be removed"
    );
    assert!(
        target.join("precious.txt").exists(),
        "the symlink target's contents must NOT be followed and deleted"
    );
}

/// 7.1: audit-triage mixed diff → one spec PR, code discarded, chatops
/// warning posted, spec branch diff is spec-only.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a43_audit_mixed_diff_opens_one_spec_pr_and_warns() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    // Executor's writes: a spec dir + an out-of-scope code file.
    write_fake_spec(&ws, "audit-fix-x");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/foo.rs"), "agent code\n").unwrap();

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"html_url":"https://github.com/owner/fixture/pull/7","number":7}"#)
        .expect(1)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let chatops = Arc::new(RecordingChatOps {
        replies: std::sync::Mutex::new(Vec::new()),
    });
    let ctx = recording_ctx(&chatops);
    let mut state = audit_state();
    let res = process_completed_triage(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &triage_github_cfg(),
        Some(&ctx),
        &mut state,
        None,
    )
    .await;
    test_hooks::set_github_api_base(None);
    res.expect("handler must succeed");

    pr_mock.assert_async().await;
    assert_eq!(
        state.status,
        crate::audits::threads::AuditThreadStatus::Acted
    );
    // Code path discarded from the working tree.
    assert!(
        !ws.join("src/foo.rs").exists(),
        "code write must be discarded"
    );
    // Spec branch carries ONLY openspec/changes/ paths.
    let files = crate::git::diff_files_changed(&ws, "main", "agent-q-triage-spec").unwrap();
    assert!(!files.is_empty(), "spec branch must carry a diff");
    assert!(
        files.iter().all(|f| f.starts_with("openspec/changes/")),
        "spec PR diff must be spec-only, got {files:?}"
    );
    let replies = chatops.replies.lock().unwrap().clone();
    assert!(
        replies
            .iter()
            .any(|r| r.contains("src/foo.rs") && r.contains("outside")),
        "a dropped-paths warning naming src/foo.rs must be posted, got {replies:?}"
    );
    assert!(
        replies.iter().any(|r| r.contains("Spec PR:")),
        "the spec PR URL must be surfaced, got {replies:?}"
    );
}

/// 7.2: chat-triage mixed diff → same shape.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a43_chat_mixed_diff_opens_one_spec_pr_and_warns() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    write_fake_spec(&ws, "chat-request-y");
    std::fs::create_dir_all(ws.join("src")).unwrap();
    std::fs::write(ws.join("src/foo.rs"), "agent code\n").unwrap();

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"html_url":"https://github.com/owner/fixture/pull/8","number":8}"#)
        .expect(1)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let chatops = Arc::new(RecordingChatOps {
        replies: std::sync::Mutex::new(Vec::new()),
    });
    let ctx = recording_ctx(&chatops);
    let mut state = proposal_state();
    let res = process_completed_proposal(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &triage_github_cfg(),
        Some(&ctx),
        &mut state,
        None,
    )
    .await;
    test_hooks::set_github_api_base(None);
    res.expect("handler must succeed");

    pr_mock.assert_async().await;
    assert_eq!(
        state.status,
        crate::proposal_requests::ProposalRequestStatus::Acted
    );
    assert!(
        !ws.join("src/foo.rs").exists(),
        "code write must be discarded"
    );
    let files = crate::git::diff_files_changed(&ws, "main", "agent-q-chat-spec").unwrap();
    assert!(
        !files.is_empty() && files.iter().all(|f| f.starts_with("openspec/changes/")),
        "spec PR diff must be spec-only, got {files:?}"
    );
    let replies = chatops.replies.lock().unwrap().clone();
    assert!(
        replies
            .iter()
            .any(|r| r.contains("src/foo.rs") && r.contains("outside")),
        "dropped-paths warning expected, got {replies:?}"
    );
}

/// 7.3: spec-only outcome → one PR, NO dropped-paths warning.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a43_audit_spec_only_opens_pr_without_warning() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    write_fake_spec(&ws, "audit-fix-z");

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"html_url":"https://github.com/owner/fixture/pull/9","number":9}"#)
        .expect(1)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let chatops = Arc::new(RecordingChatOps {
        replies: std::sync::Mutex::new(Vec::new()),
    });
    let ctx = recording_ctx(&chatops);
    let mut state = audit_state();
    let res = process_completed_triage(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &triage_github_cfg(),
        Some(&ctx),
        &mut state,
        None,
    )
    .await;
    test_hooks::set_github_api_base(None);
    res.expect("handler must succeed");

    pr_mock.assert_async().await;
    assert_eq!(
        state.status,
        crate::audits::threads::AuditThreadStatus::Acted
    );
    let replies = chatops.replies.lock().unwrap().clone();
    assert!(
        !replies.iter().any(|r| r.contains("outside")),
        "no dropped-paths warning when the agent followed the restriction, got {replies:?}"
    );
    assert!(
        replies.iter().any(|r| r.contains("Spec PR:")),
        "spec PR URL must be surfaced, got {replies:?}"
    );
}

/// 7.3 (chat): spec-only outcome → one PR, no warning.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a43_chat_spec_only_opens_pr_without_warning() {
    let (_dir, ws) = fixture_workspace_with_remote();
    let (_td, paths) = crate::testing::test_daemon_paths();
    write_fake_spec(&ws, "chat-request-z");

    let _hook = test_hooks::lock();
    let mut server = mockito::Server::new_async().await;
    let pr_mock = server
        .mock("POST", "/repos/owner/fixture/pulls")
        .with_status(201)
        .with_header("content-type", "application/json")
        .with_body(r#"{"html_url":"https://github.com/owner/fixture/pull/10","number":10}"#)
        .expect(1)
        .create_async()
        .await;
    test_hooks::set_github_api_base(Some(server.url()));

    let chatops = Arc::new(RecordingChatOps {
        replies: std::sync::Mutex::new(Vec::new()),
    });
    let ctx = recording_ctx(&chatops);
    let mut state = proposal_state();
    let res = process_completed_proposal(
        &paths,
        &ws,
        &fixture_repo(&ws),
        &triage_github_cfg(),
        Some(&ctx),
        &mut state,
        None,
    )
    .await;
    test_hooks::set_github_api_base(None);
    res.expect("handler must succeed");

    pr_mock.assert_async().await;
    assert_eq!(
        state.status,
        crate::proposal_requests::ProposalRequestStatus::Acted
    );
    let replies = chatops.replies.lock().unwrap().clone();
    assert!(
        !replies.iter().any(|r| r.contains("outside")),
        "no warning expected"
    );
}
