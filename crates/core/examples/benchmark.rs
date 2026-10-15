//! Run: cargo run --release --example benchmark
use mdwell_core::{Core, Result};
use std::fs;
use std::time::Instant;

fn main() -> Result<()> {
    let data = tempfile::tempdir()?;
    let files = tempfile::tempdir()?;
    for i in 0..1000 {
        let directory = files.path().join(format!("section-{}", i / 100));
        fs::create_dir_all(&directory)?;
        fs::write(directory.join(format!("document-{i:04}.md")), format!(
            "# Document {i}\n\n## Workspace indexing\n\nLocal search includes lunar modules and navigation.\n\n{}\n\n```rust\nlet document = {i};\n```\n",
            "Markdown content with UTF-8 café, tables and links. ".repeat(20)
        ))?;
    }
    let core = Core::open(data.path())?;
    let start = Instant::now();
    let workspace = core.add_local(files.path())?;
    println!("Initial indexing (1000 docs): {:?}", start.elapsed());
    let start = Instant::now();
    let report = core.refresh_workspace(workspace.id)?;
    println!("Unchanged refresh: {:?}, {:?}", start.elapsed(), report);
    assert_eq!(report.unchanged, 1000);
    assert_eq!(report.indexed, 0);
    let start = Instant::now();
    for _ in 0..100 {
        let hits = core.search("\"lunar modules\"", Some(workspace.id))?;
        assert_eq!(hits.len(), 50);
    }
    println!("100 phrase searches: {:?}", start.elapsed());
    let doc = core.read_document(workspace.id, "section-0/document-0000.md")?;
    let start = Instant::now();
    core.save_document(
        workspace.id,
        &doc.path,
        "# Updated\n\nUnique benchmark term.",
        &doc.revision,
    )?;
    println!(
        "Atomic save + incremental index update: {:?}",
        start.elapsed()
    );
    Ok(())
}
