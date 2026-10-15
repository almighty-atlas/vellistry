use mdwell_core::Core;
use rusqlite::{params, Connection};

#[test]
fn migration_can_recover_blocks_without_legacy_fts_and_preserves_multiple_workspaces() {
    let data = tempfile::tempdir().unwrap();
    // Empty current metadata schema, then simulate a legacy cache without FTS data.
    drop(Core::open(data.path()).unwrap());
    let db = Connection::open(data.path().join("index.sqlite3")).unwrap();
    db.execute_batch(
        "PRAGMA foreign_keys=ON;
        DROP TABLE search_schema;
        DROP TABLE search_documents;
        DROP TABLE section_fts;
        DROP TABLE search_blocks;
        DROP TABLE search_vocab;
        DROP TABLE search_signatures;
        DROP TABLE search_document_terms;",
    )
    .unwrap();
    for (id, kind, revision) in [(1, "local", "first-hash"), (2, "github", "second-hash")] {
        db.execute(
            "INSERT INTO workspaces(id,kind,name,root,url) VALUES(?1,?2,?3,?4,?5)",
            params![
                id,
                kind,
                format!("Workspace {id}"),
                format!("/unavailable/root-{id}"),
                if id == 2 {
                    Some("https://github.com/example/cached")
                } else {
                    None
                }
            ],
        )
        .unwrap();
        db.execute(
            "INSERT INTO documents VALUES(?1,'same.md','Cached title',?2)",
            params![id, revision],
        )
        .unwrap();
        db.execute(
            "INSERT INTO document_blocks VALUES(?1,'same.md',123,'Persistent quokka cache')",
            [id],
        )
        .unwrap();
    }
    let core = Core::open(data.path()).unwrap();
    assert_eq!(core.list_workspaces().unwrap().len(), 2);
    assert_eq!(core.list_documents(1).unwrap()[0].revision, "first-hash");
    assert_eq!(core.list_documents(2).unwrap()[0].revision, "second-hash");
    assert_eq!(core.search("quokka", None).unwrap().len(), 2);
    for id in [1, 2] {
        let hits = core.search("quoka", Some(id)).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].workspace_id, id);
        assert_eq!(hits[0].source_start, 123);
        assert_eq!(hits[0].source_end, None);
    }
}
