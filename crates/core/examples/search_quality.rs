//! Reproducible search corpus and 1,000-document timing harness.
use mdwell_core::Core;
use std::{fs, time::Instant};

#[path = "../tests/search_corpus.rs"]
mod corpus;
use corpus::{DOCS, QUERIES};

// Frozen pre-section SQL pipeline for this corpus only, not production search.
// All corpus queries exercise FTS; none has a legacy filename-distance hit.
fn baseline() {
    use pulldown_cmark::{Event, Parser};
    use rusqlite::{params, Connection};
    let db = Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE VIRTUAL TABLE document_fts USING fts5(headings,title,filename,path,content,workspace_id UNINDEXED,source_start UNINDEXED,tokenize='unicode61 remove_diacritics 2',prefix='2 3 4');").unwrap();
    for (path, source) in DOCS {
        let headings: Vec<_> = source
            .lines()
            .filter(|l| l.starts_with('#'))
            .map(|l| l.trim_start_matches('#').trim())
            .collect();
        let title = headings.first().copied().unwrap_or(path);
        let mut content = String::new();
        for event in Parser::new(source) {
            match event {
                Event::Text(s) | Event::Code(s) => content.push_str(&s),
                Event::End(_) | Event::SoftBreak | Event::HardBreak => content.push(' '),
                _ => {}
            }
        }
        db.execute(
            "INSERT INTO document_fts VALUES(?1,?2,?3,?3,?4,1,0)",
            params![headings.join(" \n "), title, path, content],
        )
        .unwrap();
    }
    let (mut top1, mut mrr, mut precision) = (0, 0.0, 0.0);
    for (query, expected) in QUERIES {
        let expression = if query.starts_with('"') {
            query.to_string()
        } else {
            query
                .split_whitespace()
                .map(|s| format!("\"{}\"", s.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" AND ")
        };
        let mut stmt = db.prepare("SELECT path FROM document_fts WHERE document_fts MATCH ?1 ORDER BY bm25(document_fts,12.0,10.0,8.0,6.0,1.0),path LIMIT 50").unwrap();
        let hits: Vec<String> = stmt
            .query_map([expression], |r| r.get(0))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let rank = hits.iter().position(|p| p == expected).map(|i| i + 1);
        top1 += usize::from(rank == Some(1));
        mrr += rank.map_or(0.0, |r| 1.0 / r as f64);
        precision += if hits.is_empty() {
            0.0
        } else {
            usize::from(rank.is_some()) as f64 / hits.len() as f64
        };
    }
    println!(
        "baseline snapshot: top1={:.3} MRR={:.3} mean_precision@50={:.3}",
        top1 as f64 / QUERIES.len() as f64,
        mrr / QUERIES.len() as f64,
        precision / QUERIES.len() as f64
    );
}

fn main() {
    baseline();
    let data = tempfile::tempdir().unwrap();
    let root = tempfile::tempdir().unwrap();
    for (path, source) in DOCS {
        fs::write(root.path().join(path), source).unwrap();
    }
    let core = Core::open(data.path()).unwrap();
    let workspace = core.add_local(root.path()).unwrap();
    let mut top1 = 0;
    let mut reciprocal = 0.0;
    let mut precision = 0.0;
    for (query, expected) in QUERIES {
        let hits = core.search(query, Some(workspace.id)).unwrap();
        let rank = hits.iter().position(|h| h.path == *expected).map(|i| i + 1);
        top1 += usize::from(rank == Some(1));
        reciprocal += rank.map_or(0.0, |r| 1.0 / r as f64);
        precision += if hits.is_empty() {
            0.0
        } else {
            usize::from(rank.is_some()) as f64 / hits.len() as f64
        };
        println!(
            "{query:?}: rank={rank:?} top={:?}",
            hits.first().map(|h| &h.path)
        );
    }
    println!(
        "corpus={} top1={:.3} MRR={:.3} mean_precision@50={:.3} (one relevant document/query)",
        QUERIES.len(),
        top1 as f64 / QUERIES.len() as f64,
        reciprocal / QUERIES.len() as f64,
        precision / QUERIES.len() as f64
    );
    for i in 0..1000 {
        fs::write(root.path().join(format!("fixture-{i:04}.md")), format!("# Fixture {i}\n\n## Operational guide\n\nRoutine documentation for service {i}.\n\n## Deployment\n\nDocker container maintenance and diagnostics {i}." )).unwrap();
    }
    let start = Instant::now();
    let indexed = core.refresh_workspace(workspace.id).unwrap();
    println!("1000doc index: {:?} {indexed:?}", start.elapsed());
    for query in [
        "Docker",
        "OAuth refresh",
        "doker compsoe",
        "Zugriffsschlüssel",
        "\"lunar module\"",
    ] {
        let mut times = Vec::new();
        for _ in 0..100 {
            let start = Instant::now();
            core.search(query, Some(workspace.id)).unwrap();
            times.push(start.elapsed().as_micros());
        }
        times.sort();
        println!(
            "1000doc search {query:?}: p50={}us p95={}us",
            times[50], times[95]
        );
    }
    fs::write(
        root.path().join("fixture-0000.md"),
        "# Updated\n\n## OAuth\n\nOIDC credentials.",
    )
    .unwrap();
    let start = Instant::now();
    println!(
        "incremental {:?} elapsed={:?}",
        core.refresh_workspace(workspace.id).unwrap(),
        start.elapsed()
    );
    drop(core);
    let db = rusqlite::Connection::open(data.path().join("index.sqlite3")).unwrap();
    db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)").unwrap();
    println!(
        "index bytes={}",
        fs::metadata(data.path().join("index.sqlite3"))
            .unwrap()
            .len()
    );
}
