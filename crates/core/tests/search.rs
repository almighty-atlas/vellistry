mod search_corpus;
use mdwell_core::Core;
use rusqlite::{params, Connection};
use std::fs;

#[test]
fn query_corpus_top1_and_phrase_semantics() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    for (path, source) in search_corpus::DOCS {
        fs::write(files.path().join(path), source).unwrap();
    }
    let core = Core::open(data.path()).unwrap();
    let ws = core.add_local(files.path()).unwrap();
    for (query, expected) in search_corpus::QUERIES {
        let hits = core.search(query, Some(ws.id)).unwrap();
        assert_eq!(
            hits.first().map(|h| h.path.as_str()),
            Some(*expected),
            "{query}"
        );
    }
    for query in [
        "\"doker compose\"",
        "\"module lunar\"",
        "\"oatuh odic\"",
        "doker missingtechnicalword",
        "OAuth missingtechnicalword",
    ] {
        assert!(
            core.search(query, Some(ws.id)).unwrap().is_empty(),
            "{query}"
        );
    }
    let hits = core.search("doker compsoe", None).unwrap();
    assert!(hits[0].matched_terms.contains(&"docker".to_string()));
    assert!(hits[0].matched_terms.contains(&"compose".to_string()));
    // Acronyms stay searchable and punctuation segments into AND terms.
    assert_eq!(core.search("OIDC", None).unwrap()[0].path, "oauth.md");
    assert_eq!(core.search("ci/c", None).unwrap()[0].path, "actions.md");
    assert!(core.search(&"OAuth ".repeat(17), None).unwrap().is_empty());
    assert!(core.search(&"a".repeat(1025), None).unwrap().is_empty());
    // Suffix/substring matching is deliberately not implicit.
    assert!(core.search("schlüsselverwaltung", None).unwrap().is_empty());
}

#[test]
fn inherited_sections_navigation_utf8_and_no_cross_section_and() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    let source = "---\ntitle: ignored\n---\n# Héllo\n\nIntro.\n\n## OAuth\n\nUnicorn.\n\n### Refresh tokens\n\nÉléphant **credentials** are rotated.\n\n## Other\n\nNarwhal.\n";
    fs::write(files.path().join("hierarchy.md"), source).unwrap();
    let long_code = format!(
        "# Code\n\n```text\n{}\nuniquetarget\n```\n",
        "ordinary line\n".repeat(1000)
    );
    fs::write(files.path().join("code.md"), &long_code).unwrap();
    fs::write(
        files.path().join("inline-split.md"),
        "# Rendered\n\nThe **lun**ar [mod](https://example.com)ule is ready.\n",
    )
    .unwrap();
    let core = Core::open(data.path()).unwrap();
    let ws = core.add_local(files.path()).unwrap();
    let hit = core
        .search("OAuth credentials", Some(ws.id))
        .unwrap()
        .remove(0);
    assert_eq!(hit.section_title, "Héllo / OAuth / Refresh tokens");
    assert_eq!(hit.source_start, source.find("Éléphant").unwrap());
    assert!(source.is_char_boundary(hit.source_start));
    assert!(source.is_char_boundary(hit.source_end.unwrap()));
    assert!(source[hit.source_start..hit.source_end.unwrap()].contains("**credentials**"));
    assert!(!hit.snippet.contains("**"));
    assert!(core.search("unicorn narwhal", None).unwrap().is_empty());
    assert_eq!(
        core.search("\"lunar module\"", None).unwrap()[0].path,
        "inline-split.md"
    );
    assert!(hit.matched_terms.contains(&"oauth".to_string()));
    let hit = core.search("uniquetarget", None).unwrap().remove(0);
    assert_eq!(hit.source_start, long_code.find("```text").unwrap());
    assert!(hit.snippet.contains("uniquetarget"));
    assert!(hit.snippet.chars().count() < 500);
    let db = Connection::open(data.path().join("index.sqlite3")).unwrap();
    let count: i64 = db
        .query_row(
            "SELECT count(*) FROM section_fts WHERE path='hierarchy.md'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 4);
    let content: String = db
        .query_row(
            "SELECT content FROM section_fts WHERE section_title='Héllo / OAuth / Refresh tokens'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!content.contains("Refresh tokens"));
    assert!(content.contains("credentials"));
}

#[test]
fn exact_filename_tier_dedup_50_determinism_and_incremental_cleanup() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    fs::write(files.path().join("README.md"), "# Plain\n\nWelcome.").unwrap();
    for i in 0..70 {
        fs::write(
            files.path().join(format!("noise-{i:03}.md")),
            "# README\n\n## README\n\nREADME README.\n\n## README again\n\nREADME.",
        )
        .unwrap();
    }
    let core = Core::open(data.path()).unwrap();
    let ws = core.add_local(files.path()).unwrap();
    let first = core.search("readme", None).unwrap();
    assert_eq!(first.len(), 50);
    assert_eq!(first[0].path, "README.md");
    assert_eq!(first, core.search("readme", None).unwrap());
    let paths: std::collections::HashSet<_> = first.iter().map(|h| &h.path).collect();
    assert_eq!(paths.len(), 50);
    let doc = core.read_document(ws.id, "README.md").unwrap();
    core.save_document(
        ws.id,
        "README.md",
        "# Replacement\n\nUniqueplatypus.",
        &doc.revision,
    )
    .unwrap();
    assert_eq!(
        core.search("uniqueplatypu", None).unwrap()[0].path,
        "README.md"
    );
    fs::remove_file(files.path().join("README.md")).unwrap();
    core.refresh_workspace(ws.id).unwrap();
    assert!(core.search("uniqueplatypu", None).unwrap().is_empty());
    assert!(core.search("uniqueplatypus", None).unwrap().is_empty());
    let db = Connection::open(data.path().join("index.sqlite3")).unwrap();
    let orphaned: i64 = db.query_row("SELECT count(*) FROM search_blocks b WHERE NOT EXISTS(SELECT 1 FROM section_fts f WHERE f.rowid=b.section_id)",[],|r|r.get(0)).unwrap();
    assert_eq!(orphaned, 0);
    let signatures: i64 = db
        .query_row(
            "SELECT count(*) FROM search_signatures WHERE term='uniqueplatypus'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(signatures, 0);
}

fn legacy(data: &std::path::Path, root: &std::path::Path) {
    let db = Connection::open(data.join("index.sqlite3")).unwrap();
    db.execute_batch("PRAGMA foreign_keys=ON;
        CREATE TABLE workspaces(id INTEGER PRIMARY KEY,kind TEXT NOT NULL,name TEXT NOT NULL,root TEXT NOT NULL UNIQUE,url TEXT UNIQUE);
        CREATE TABLE documents(workspace_id INTEGER NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,path TEXT NOT NULL,title TEXT NOT NULL,revision TEXT NOT NULL,PRIMARY KEY(workspace_id,path));
        CREATE TABLE document_blocks(workspace_id INTEGER NOT NULL,path TEXT NOT NULL,source_start INTEGER NOT NULL,content TEXT NOT NULL,PRIMARY KEY(workspace_id,path,source_start),FOREIGN KEY(workspace_id,path) REFERENCES documents(workspace_id,path) ON DELETE CASCADE);
        CREATE VIRTUAL TABLE document_fts USING fts5(headings,title,filename,path,content,workspace_id UNINDEXED,source_start UNINDEXED,tokenize='unicode61 remove_diacritics 2',prefix='2 3 4');
        ").unwrap();
    db.execute("INSERT INTO workspaces VALUES(7,'github','Private metadata',?1,'https://github.com/example/private')",[root.to_str().unwrap()]).unwrap();
    db.execute_batch("INSERT INTO documents VALUES(7,'cached.md','Old title','preserved-revision');
        INSERT INTO document_blocks VALUES(7,'cached.md',42,'Cached OAuth credentials.');
        INSERT INTO document_fts VALUES('OAuth / Tokens','Old title','cached.md','cached.md','Cached OAuth credentials.',7,0);").unwrap();
}

#[test]
fn legacy_migration_offline_preserves_metadata_and_revision_then_rebuilds() {
    let data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("offline-root");
    legacy(data.path(), &root);
    let core = Core::open(data.path()).unwrap();
    let workspace = core.list_workspaces().unwrap().remove(0);
    assert_eq!(workspace.id, 7);
    assert_eq!(workspace.name, "Private metadata");
    assert_eq!(
        workspace.url.as_deref(),
        Some("https://github.com/example/private")
    );
    assert_eq!(
        core.list_documents(7).unwrap()[0].revision,
        "preserved-revision"
    );
    let hit = core.search("OAuth credentials", Some(7)).unwrap().remove(0);
    assert_eq!(hit.source_start, 42);
    assert_eq!(hit.source_end, None);
    assert!(core.search("tokens", Some(7)).unwrap()[0]
        .section_title
        .contains("Tokens"));
    assert!(core.refresh_workspace(7).is_err());
    assert_eq!(core.search("credentials", None).unwrap().len(), 1);
    drop(core);
    let core = Core::open(data.path()).unwrap();
    assert_eq!(core.search("credentials", None).unwrap().len(), 1);
    fs::create_dir(&root).unwrap();
    let source = "# OAuth\n\n## Tokens\n\nCached OAuth credentials.";
    fs::write(root.join("cached.md"), source).unwrap();
    // Make the cached revision equal the disk hash: the rebuild marker must still win.
    let revision = core.read_document(7, "cached.md").unwrap().revision;
    let db = Connection::open(data.path().join("index.sqlite3")).unwrap();
    db.execute(
        "UPDATE documents SET revision=?1 WHERE workspace_id=7",
        [revision],
    )
    .unwrap();
    let report = core.refresh_workspace(7).unwrap();
    assert_eq!(report.indexed, 1);
    let hit = core.search("credentials", Some(7)).unwrap().remove(0);
    assert_eq!(hit.section_title, "OAuth / Tokens");
    assert_eq!(hit.source_start, source.find("Cached").unwrap());
    assert!(hit.source_end.is_some());
    assert_eq!(core.refresh_workspace(7).unwrap().unchanged, 1);
    let legacy_table: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='document_fts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(legacy_table, 0);
}

#[test]
fn migration_failure_rolls_back_and_future_versions_are_not_destroyed() {
    let data = tempfile::tempdir().unwrap();
    legacy(data.path(), std::path::Path::new("/missing-root"));
    let db = Connection::open(data.path().join("index.sqlite3")).unwrap();
    db.execute_batch("CREATE TABLE search_signatures(conflicting_schema TEXT);")
        .unwrap();
    assert!(Core::open(data.path()).is_err());
    let docs: String = db
        .query_row("SELECT revision FROM documents", [], |r| r.get(0))
        .unwrap();
    assert_eq!(docs, "preserved-revision");
    let count: i64 = db
        .query_row("SELECT count(*) FROM document_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    let section_table: i64 = db
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='section_fts'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(section_table, 0);
    db.execute_batch("DROP TABLE search_signatures;").unwrap();
    drop(Core::open(data.path()).unwrap());
    db.execute("UPDATE search_schema SET version=?1", params![999])
        .unwrap();
    assert!(Core::open(data.path()).is_err());
    let count: i64 = db
        .query_row("SELECT count(*) FROM section_fts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}
