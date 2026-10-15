use mdwell_core::{Core, Error, MAX_DOCUMENT_BYTES};
use std::fs;

#[test]
fn failed_refresh_is_transactional_and_reads_are_bounded_utf8() {
    let data = tempfile::tempdir().unwrap();
    let files = tempfile::tempdir().unwrap();
    fs::write(files.path().join("good.md"), "# Good\n\nold snapshot").unwrap();
    let core = Core::open(data.path()).unwrap();
    let workspace = core.add_local(files.path()).unwrap();
    fs::write(files.path().join("good.md"), "# Changed\n\nnew snapshot").unwrap();
    let huge = files.path().join("oversized.md");
    fs::File::create(&huge)
        .unwrap()
        .set_len(MAX_DOCUMENT_BYTES + 1)
        .unwrap();
    assert!(matches!(
        core.refresh_workspace(workspace.id),
        Err(Error::TooLarge(_))
    ));
    assert!(matches!(
        core.read_document(workspace.id, "oversized.md"),
        Err(Error::TooLarge(_))
    ));
    assert_eq!(core.list_documents(workspace.id).unwrap()[0].title, "Good");
    fs::remove_file(huge).unwrap();
    fs::write(files.path().join("invalid.md"), [0xff, 0xfe]).unwrap();
    assert!(matches!(
        core.refresh_workspace(workspace.id),
        Err(Error::Utf8(_))
    ));
    assert!(matches!(
        core.read_document(workspace.id, "invalid.md"),
        Err(Error::Utf8(_))
    ));
    assert_eq!(core.list_documents(workspace.id).unwrap()[0].title, "Good");
    fs::remove_file(files.path().join("invalid.md")).unwrap();
    assert_eq!(core.refresh_workspace(workspace.id).unwrap().indexed, 1);
    assert_eq!(
        core.list_documents(workspace.id).unwrap()[0].title,
        "Changed"
    );
}
