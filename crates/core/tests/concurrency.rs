use mdwell_core::{Core, Error};
use std::fs;
use std::sync::Barrier;

#[test]
fn independent_instances_cannot_both_save_the_same_revision() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    fs::write(files.path().join("doc.md"), "# Original").unwrap();
    let first = Core::open(data.path()).unwrap();
    let workspace = first.add_local(files.path()).unwrap();
    let second = Core::open(data.path()).unwrap();
    let revision = first
        .read_document(workspace.id, "doc.md")
        .unwrap()
        .revision;
    let barrier = Barrier::new(2);
    let (a, b) = std::thread::scope(|scope| {
        let a = scope.spawn(|| {
            barrier.wait();
            first.save_document(workspace.id, "doc.md", "# Writer A", &revision)
        });
        let b = scope.spawn(|| {
            barrier.wait();
            second.save_document(workspace.id, "doc.md", "# Writer B", &revision)
        });
        (a.join().unwrap(), b.join().unwrap())
    });
    assert_eq!(usize::from(a.is_ok()) + usize::from(b.is_ok()), 1);
    let losing = if a.is_err() { a } else { b };
    assert!(matches!(losing, Err(Error::Conflict { .. })));
    let current = first.read_document(workspace.id, "doc.md").unwrap();
    let indexed = second.list_documents(workspace.id).unwrap();
    assert_eq!(indexed[0].revision, current.revision);
    assert_eq!(indexed[0].title, current.title);
}

#[test]
fn conflict_errors_have_a_roundtrippable_transport_representation() {
    let error = Error::Conflict {
        expected: "old".into(),
        actual: "new".into(),
    };
    let json = serde_json::to_string(&error).unwrap();
    let info: mdwell_core::ErrorInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(info.code, mdwell_core::ErrorCode::Conflict);
    assert_eq!(info.expected_revision.as_deref(), Some("old"));
    assert_eq!(info.actual_revision.as_deref(), Some("new"));
    assert_eq!(info, error.info());
}
