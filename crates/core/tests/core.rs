use mdwell_core::{Core, Error, ResolvedLink, MAX_ASSET_BYTES};
use std::fs;
use std::path::Path;

fn write(root: &Path, path: &str, content: &str) {
    let target = root.join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, content).unwrap();
}

#[test]
fn persistence_incremental_refresh_and_workspace_isolation() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    write(
        files.path(),
        "nested/guide.markdown",
        "# Guide\n\nPersistent quokka.",
    );
    write(files.path(), "UPPER.MD", "# Upper");
    for dir in [".git", "node_modules", "target"] {
        write(files.path(), &format!("{dir}/hidden.md"), "hidden quokka");
    }
    write(files.path(), "other.txt", "not indexed");
    let core = Core::open(data.path()).unwrap();
    let workspace = core.add_local(files.path()).unwrap();
    assert_eq!(core.add_local(files.path()).unwrap().id, workspace.id);
    assert_eq!(core.list_documents(workspace.id).unwrap().len(), 2);
    let report = core.refresh_workspace(workspace.id).unwrap();
    assert_eq!(
        (report.indexed, report.unchanged, report.removed),
        (0, 2, 0)
    );
    let second_files = tempfile::tempdir().unwrap();
    write(second_files.path(), "second.md", "quokka");
    let second = core.add_local(second_files.path()).unwrap();
    assert_eq!(core.search("quokka", None).unwrap().len(), 2);
    assert_eq!(
        core.search("quokka", Some(second.id)).unwrap()[0].path,
        "second.md"
    );
    drop(core);
    let core = Core::open(data.path()).unwrap();
    assert_eq!(core.list_workspaces().unwrap().len(), 2);
    assert_eq!(core.search("quokka", Some(workspace.id)).unwrap().len(), 1);
    write(
        files.path(),
        "nested/guide.markdown",
        "# Changed\n\nPlatypus.",
    );
    fs::remove_file(files.path().join("UPPER.MD")).unwrap();
    let report = core.refresh_workspace(workspace.id).unwrap();
    assert_eq!(
        (report.indexed, report.unchanged, report.removed),
        (1, 0, 1)
    );
    assert!(core
        .search("quokka", Some(workspace.id))
        .unwrap()
        .is_empty());
    assert_eq!(
        core.search("platypus", Some(workspace.id)).unwrap()[0].title,
        "Changed"
    );
    assert!(matches!(
        core.list_documents(99999),
        Err(Error::WorkspaceNotFound(_))
    ));
    assert!(serde_json::to_string(&core.list_workspaces().unwrap())
        .unwrap()
        .contains("local"));
}

#[test]
fn rendering_gfm_sanitization_unique_ids_and_byte_offsets() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = "# Héllo *world*\n\nA naïve paragraph.\n\n## Repeat\n\n## Repeat\n\n## Repeat-1\n\n| A | B |\n|---|---|\n| x | y |\n\n~~gone~~\n\n- [x] done\n\n```rust\nlet x = 1;\n```\n\n<script>alert(1)</script>\n\n<img src=\"https://tracker.invalid/x\" onerror=\"alert(2)\">\n\n![remote](https://tracker.invalid/pixel.png)\n\n![local](img.png)\n\n[bad](javascript:alert(3))\n";
    write(files.path(), "doc.md", source);
    let core = Core::open(data.path()).unwrap();
    let ws = core.add_local(files.path()).unwrap();
    let doc = core.read_document(ws.id, "doc.md").unwrap();
    assert_eq!(doc.title, "Héllo world");
    assert_eq!(
        doc.toc.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
        ["héllo-world", "repeat", "repeat-1", "repeat-1-1"]
    );
    assert!(doc.html.contains("id=\"héllo-world\""));
    assert!(doc.html.contains("<table>"));
    assert!(doc.html.contains("<del>gone</del>"));
    assert!(doc.html.contains("☑"));
    assert!(doc.html.contains("<pre><code"));
    assert!(!doc.html.contains("<script"));
    assert_eq!(doc.html.matches("<img ").count(), 1);
    assert!(doc.html.contains("data-local-src=\"img.png\""));
    assert!(!doc.html.contains(" src="));
    assert!(!doc.html.contains("srcset="));
    assert!(!doc.html.contains("tracker.invalid"));
    assert!(!doc.html.contains("onerror"));
    assert!(!doc.html.contains("javascript:"));
    let start = source.find("A naïve").unwrap();
    assert!(doc.html.contains(&format!("data-source-start=\"{start}\"")));
    let hit = core.search("naïve", Some(ws.id)).unwrap().remove(0);
    assert_eq!(hit.source_start, start);
    assert_eq!(
        core.search("naive", Some(ws.id)).unwrap()[0].source_start,
        start
    );
    assert_eq!(&source[hit.source_start..hit.source_start + 1], "A");
    assert!(hit.snippet.contains("naïve"));
    assert!(!hit.snippet.contains("<mark>"));
    assert_eq!(doc.toc[1].source_start, source.find("## Repeat").unwrap());
}

#[test]
fn search_ranking_phrases_prefix_fuzzy_and_literal_queries() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    write(files.path(), "title.md", "# Astronaut\n\nShort text.");
    write(files.path(), "body.md", "# Ordinary\n\nAstronaut text.");
    write(
        files.path(),
        "navigation.md",
        "# Manual\n\nThe lunar module is operational.",
    );
    write(files.path(), "order.md", "# Other\n\nThe module is lunar.");
    write(
        files.path(),
        "headings.md",
        "# Manual\n\n## Telescope\n\nNotes.",
    );
    write(files.path(), "content.md", "# Manual\n\nTelescope notes.");
    write(
        files.path(),
        "spread.md",
        "# Spread\n\nUnicorn first.\n\nNarwhal second.",
    );
    let core = Core::open(data.path()).unwrap();
    let ws = core.add_local(files.path()).unwrap();
    let hits = core.search("astronaut", Some(ws.id)).unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].path, "title.md");
    assert_eq!(
        core.search("telescope", None).unwrap()[0].path,
        "headings.md"
    );
    assert_eq!(core.search("\"lunar module\"", None).unwrap().len(), 1);
    assert_eq!(
        core.search("operat*", None).unwrap()[0].path,
        "navigation.md"
    );
    assert_eq!(
        core.search("navigaton", None).unwrap()[0].path,
        "navigation.md"
    );
    assert_eq!(
        core.search("unicorn narwhal", None).unwrap()[0].path,
        "spread.md"
    );
    for query in [
        "",
        "***",
        "\"",
        "OR NOT NEAR",
        "\" OR *",
        "{broken}",
        "a:b",
        "(x)",
    ] {
        assert!(core.search(query, None).is_ok(), "{query}");
    }
}

#[test]
fn atomic_saves_conflicts_index_updates_and_permissions() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    write(files.path(), "doc.md", "# Before\n\noriginal");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            files.path().join("doc.md"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
    }
    let core = Core::open(data.path()).unwrap();
    let ws = core.add_local(files.path()).unwrap();
    let before = core.read_document(ws.id, "doc.md").unwrap();
    let after = core
        .save_document(ws.id, "doc.md", "# After\n\nreplacement", &before.revision)
        .unwrap();
    assert_ne!(before.revision, after.revision);
    assert_eq!(
        fs::read_to_string(files.path().join("doc.md")).unwrap(),
        after.content
    );
    assert_eq!(core.search("replacement", None).unwrap().len(), 1);
    assert!(core.search("original", None).unwrap().is_empty());
    assert!(matches!(
        core.save_document(ws.id, "doc.md", "bad", &before.revision),
        Err(Error::Conflict { .. })
    ));
    write(files.path(), "doc.md", "external edit");
    assert!(matches!(
        core.save_document(ws.id, "doc.md", "bad", &after.revision),
        Err(Error::Conflict { .. })
    ));
    assert_eq!(
        fs::read_to_string(files.path().join("doc.md")).unwrap(),
        "external edit"
    );
    assert!(core.save_document(ws.id, "missing.md", "new", "").is_err());
    assert!(!files.path().join("missing.md").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(files.path().join("doc.md"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }
    // A conflict must not modify the index, either.
    assert_eq!(core.search("replacement", None).unwrap().len(), 1);
    assert_eq!(fs::read_dir(files.path()).unwrap().count(), 1);
}

#[test]
fn safe_links_assets_limits_and_rejected_paths() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    write(files.path(), "nested/doc.md", "# Doc");
    write(files.path(), "other space.md", "# Other");
    write(files.path(), "image.png", "fixture image bytes");
    write(files.path(), "vector.svg", "<svg onload='alert(1)'/>");
    write(files.path(), ".git/secret.md", "secret");
    let core = Core::open(data.path()).unwrap();
    let ws = core.add_local(files.path()).unwrap();
    assert_eq!(
        core.resolve_link(ws.id, "nested/doc.md", "../other%20space.md#part")
            .unwrap(),
        ResolvedLink::Document {
            path: "other space.md".into(),
            fragment: Some("part".into())
        }
    );
    assert_eq!(
        core.resolve_link(ws.id, "nested/doc.md", "#top").unwrap(),
        ResolvedLink::Document {
            path: "nested/doc.md".into(),
            fragment: Some("top".into())
        }
    );
    assert!(matches!(
        core.resolve_link(ws.id, "nested/doc.md", "../image.png"),
        Ok(ResolvedLink::Asset { .. })
    ));
    assert!(matches!(
        core.resolve_link(ws.id, "nested/doc.md", "https://example.com"),
        Ok(ResolvedLink::External { .. })
    ));
    for href in [
        "javascript:alert(1)",
        "data:text/html,test",
        "//evil.test/x",
        "../../escape.md",
        "../%2e%2e/escape.md",
        "../.git/secret.md",
        "../vector.svg",
        "file:///etc/passwd",
    ] {
        assert!(
            core.resolve_link(ws.id, "nested/doc.md", href).is_err(),
            "{href}"
        );
    }
    for path in [
        "../outside.md",
        "/etc/passwd",
        ".git/secret.md",
        "nested\\doc.md",
        "nested/./doc.md",
        "nested//doc.md",
        "nested/../../outside.md",
    ] {
        assert!(core.read_document(ws.id, path).is_err(), "{path}");
        assert!(
            core.save_document(ws.id, path, "oops", "").is_err(),
            "{path}"
        );
        assert!(core.read_asset(ws.id, path).is_err(), "{path}");
    }
    let image = core.read_asset(ws.id, "image.png").unwrap();
    assert_eq!(image.mime_type, "image/png");
    assert_eq!(image.bytes, b"fixture image bytes");
    assert!(core.read_asset(ws.id, "vector.svg").is_err());
    fs::File::create(files.path().join("huge.png"))
        .unwrap()
        .set_len(MAX_ASSET_BYTES + 1)
        .unwrap();
    assert!(matches!(
        core.read_asset(ws.id, "huge.png"),
        Err(Error::TooLarge(_))
    ));
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_escape_or_bypass_exclusions() {
    use std::os::unix::fs::symlink;
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write(outside.path(), "secret.md", "classified");
    write(outside.path(), "secret.png", "private");
    write(files.path(), "real.md", "# Safe");
    write(files.path(), ".git/secret.md", "git secret");
    symlink(
        outside.path().join("secret.md"),
        files.path().join("escape.md"),
    )
    .unwrap();
    symlink(
        outside.path().join("secret.png"),
        files.path().join("escape.png"),
    )
    .unwrap();
    symlink(outside.path(), files.path().join("directory")).unwrap();
    symlink(
        files.path().join(".git/secret.md"),
        files.path().join("alias.md"),
    )
    .unwrap();
    symlink(files.path().join("real.md"), files.path().join("inside.md")).unwrap();
    let core = Core::open(data.path()).unwrap();
    let ws = core.add_local(files.path()).unwrap();
    assert_eq!(core.list_documents(ws.id).unwrap().len(), 2);
    assert!(core.search("classified", None).unwrap().is_empty());
    for path in ["escape.md", "directory/secret.md", "alias.md"] {
        assert!(matches!(
            core.read_document(ws.id, path),
            Err(Error::UnsafePath(_))
        ));
        assert!(core.save_document(ws.id, path, "bad", "").is_err());
        assert!(core.resolve_link(ws.id, "real.md", path).is_err());
    }
    assert!(core.read_asset(ws.id, "escape.png").is_err());
    let doc = core.read_document(ws.id, "inside.md").unwrap();
    core.save_document(ws.id, "inside.md", "# Updated", &doc.revision)
        .unwrap();
    assert!(fs::symlink_metadata(files.path().join("inside.md"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read_to_string(files.path().join("real.md")).unwrap(),
        "# Updated"
    );
    assert!(core
        .list_documents(ws.id)
        .unwrap()
        .iter()
        .all(|doc| doc.title == "Updated"));
    // A formerly valid link retargeted outside is removed by refresh.
    fs::remove_file(files.path().join("inside.md")).unwrap();
    symlink(
        outside.path().join("secret.md"),
        files.path().join("inside.md"),
    )
    .unwrap();
    assert_eq!(core.refresh_workspace(ws.id).unwrap().removed, 1);
    assert_eq!(
        fs::read_to_string(outside.path().join("secret.md")).unwrap(),
        "classified"
    );
}

#[test]
fn github_api_rejects_non_public_https_before_network() {
    let data = tempfile::tempdir().unwrap();
    let core = Core::open(data.path()).unwrap();
    for url in [
        "file:///tmp/repo",
        "https://github.com/a/b?token=secret",
        "https://token@github.com/a/b",
        "https://github.com.evil.test/a/b",
        "--upload-pack=bad",
    ] {
        assert!(matches!(
            core.add_github(url),
            Err(Error::InvalidGithubUrl(_))
        ));
    }
    assert!(core.list_workspaces().unwrap().is_empty());
}
