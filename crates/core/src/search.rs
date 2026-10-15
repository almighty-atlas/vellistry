//! SQLite-only section search. All writes run in the caller's index transaction.
use crate::{markdown::Rendered, Result, SearchHit};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use std::{collections::BTreeSet, path::Path};
use unicode_normalization::{char::is_combining_mark, UnicodeNormalization};

const VERSION: i64 = 1;
const CANDIDATES: usize = 128;

pub(crate) fn fold(text: &str) -> String {
    text.nfd()
        .filter(|c| !is_combining_mark(*c))
        .flat_map(char::to_lowercase)
        .collect()
}
fn words(text: &str) -> Vec<String> {
    fold(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
fn quoted(text: &str) -> String {
    format!("\"{}\"", text.replace('"', "\"\""))
}

pub(crate) fn migrate(db: &mut Connection) -> Result<()> {
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    tx.execute_batch("CREATE TABLE IF NOT EXISTS search_schema(version INTEGER NOT NULL);")?;
    let version: i64 = tx
        .query_row("SELECT version FROM search_schema", [], |r| r.get(0))
        .optional()?
        .unwrap_or(0);
    if version > VERSION {
        return Err(rusqlite::Error::InvalidQuery.into());
    }
    if version == VERSION {
        tx.commit()?;
        return Ok(());
    }
    tx.execute_batch("CREATE VIRTUAL TABLE IF NOT EXISTS document_fts USING fts5(
        headings,title,filename,path,content,workspace_id UNINDEXED,source_start UNINDEXED,
        tokenize='unicode61 remove_diacritics 2',prefix='2 3 4');
        CREATE TABLE search_documents(
        workspace_id INTEGER NOT NULL, path TEXT NOT NULL, filename TEXT NOT NULL, stem TEXT NOT NULL,
        rebuild INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(workspace_id,path),
        FOREIGN KEY(workspace_id,path) REFERENCES documents(workspace_id,path) ON DELETE CASCADE);
        CREATE INDEX search_filename ON search_documents(filename);
        CREATE INDEX search_stem ON search_documents(stem);
        CREATE VIRTUAL TABLE section_fts USING fts5(headings,title,filename,path,content,
          workspace_id UNINDEXED,source_start UNINDEXED,source_end UNINDEXED,section_title UNINDEXED,
          tokenize='unicode61 remove_diacritics 2',prefix='2 3 4');
        CREATE VIRTUAL TABLE search_blocks USING fts5(content,section_id,
          source_start UNINDEXED,source_end UNINDEXED,tokenize='unicode61 remove_diacritics 2');
        CREATE VIRTUAL TABLE search_vocab USING fts5vocab(section_fts,'row');
        CREATE TABLE search_signatures(signature TEXT NOT NULL,term TEXT NOT NULL,PRIMARY KEY(signature,term)) WITHOUT ROWID;
        CREATE TABLE search_document_terms(workspace_id INTEGER NOT NULL,path TEXT NOT NULL,term TEXT NOT NULL,
          PRIMARY KEY(workspace_id,path,term),
          FOREIGN KEY(workspace_id,path) REFERENCES documents(workspace_id,path) ON DELETE CASCADE);
        CREATE INDEX search_terms_term ON search_document_terms(term);")?;
    let legacy = {
        let mut stmt = tx.prepare("SELECT d.workspace_id,d.path,d.title,coalesce(f.headings,''),coalesce(f.content,'')
            FROM documents d LEFT JOIN document_fts f ON f.workspace_id=d.workspace_id AND f.path=d.path ORDER BY d.workspace_id,d.path")?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    for (workspace, path, title, headings, content) in legacy {
        metadata(&tx, workspace, &path, true)?;
        let blocks = {
            let mut stmt = tx.prepare("SELECT source_start,content FROM document_blocks WHERE workspace_id=?1 AND path=?2 ORDER BY source_start")?;
            let rows = stmt.query_map(params![workspace, path], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
            })?;
            rows.collect::<std::result::Result<Vec<_>, _>>()?
        };
        let body = if content.is_empty() {
            blocks
                .iter()
                .map(|b| b.1.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            content
        };
        let id = section(
            &tx,
            workspace,
            &path,
            &title,
            &headings,
            &body,
            blocks.first().map_or(0, |b| b.0),
            None,
            &headings,
        )?;
        for (start, text) in blocks {
            block(&tx, id, start, None, &text)?;
        }
        add_terms(
            &tx,
            workspace,
            &path,
            &format!("{path} {title} {headings} {body}"),
        )?;
    }
    // The old cache is no longer needed. Metadata and revisions are never reset.
    tx.execute_batch("DROP TABLE document_fts; INSERT INTO search_schema VALUES(1);")?;
    tx.commit()?;
    Ok(())
}

pub(crate) fn needs_rebuild(db: &Connection, workspace: i64, path: &str) -> Result<bool> {
    Ok(db
        .query_row(
            "SELECT rebuild FROM search_documents WHERE workspace_id=?1 AND path=?2",
            params![workspace, path],
            |r| r.get::<_, bool>(0),
        )
        .optional()?
        .unwrap_or(true))
}
fn metadata(db: &Connection, workspace: i64, path: &str, rebuild: bool) -> Result<()> {
    let filename = Path::new(path)
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or(path);
    let stem = Path::new(path)
        .file_stem()
        .and_then(|v| v.to_str())
        .unwrap_or(path);
    db.execute("INSERT OR REPLACE INTO search_documents(workspace_id,path,filename,stem,rebuild) VALUES(?1,?2,?3,?4,?5)", params![workspace,path,fold(filename),fold(stem),rebuild])?;
    Ok(())
}
pub(crate) fn remove(db: &Connection, workspace: i64, path: &str) -> Result<()> {
    db.execute("DELETE FROM search_blocks WHERE section_id IN (SELECT rowid FROM section_fts WHERE workspace_id=?1 AND path=?2)", params![workspace,path])?;
    db.execute(
        "DELETE FROM section_fts WHERE workspace_id=?1 AND path=?2",
        params![workspace, path],
    )?;
    let terms = {
        let mut stmt =
            db.prepare("SELECT term FROM search_document_terms WHERE workspace_id=?1 AND path=?2")?;
        let rows = stmt.query_map(params![workspace, path], |r| r.get::<_, String>(0))?;
        rows.collect::<std::result::Result<Vec<_>, _>>()?
    };
    db.execute(
        "DELETE FROM search_document_terms WHERE workspace_id=?1 AND path=?2",
        params![workspace, path],
    )?;
    for term in terms {
        db.execute("DELETE FROM search_signatures WHERE term=?1 AND NOT EXISTS(SELECT 1 FROM search_document_terms WHERE term=?1)",[term])?;
    }
    db.execute(
        "DELETE FROM search_documents WHERE workspace_id=?1 AND path=?2",
        params![workspace, path],
    )?;
    Ok(())
}
fn signatures(term: &str) -> BTreeSet<String> {
    let chars: Vec<_> = term.chars().collect();
    let mut result = BTreeSet::from([term.to_owned()]);
    for i in 0..chars.len() {
        result.insert(
            chars
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != i)
                .map(|(_, c)| c)
                .collect(),
        );
    }
    result
}
fn add_terms(db: &Connection, workspace: i64, path: &str, text: &str) -> Result<()> {
    for term in words(text).into_iter().collect::<BTreeSet<_>>() {
        db.execute(
            "INSERT OR IGNORE INTO search_document_terms VALUES(?1,?2,?3)",
            params![workspace, path, term],
        )?;
        if !(3..=64).contains(&term.chars().count()) {
            continue;
        }
        for signature in signatures(&term) {
            db.execute(
                "INSERT OR IGNORE INTO search_signatures VALUES(?1,?2)",
                params![signature, term],
            )?;
        }
    }
    Ok(())
}
#[allow(clippy::too_many_arguments)]
fn section(
    db: &Connection,
    workspace: i64,
    path: &str,
    title: &str,
    headings: &str,
    content: &str,
    start: i64,
    end: Option<i64>,
    section_title: &str,
) -> Result<i64> {
    let filename = Path::new(path)
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or(path);
    db.execute("INSERT INTO section_fts(headings,title,filename,path,content,workspace_id,source_start,source_end,section_title) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)", params![headings,title,filename,path,content,workspace,start,end,section_title])?;
    Ok(db.last_insert_rowid())
}
fn block(db: &Connection, section: i64, start: i64, end: Option<i64>, text: &str) -> Result<()> {
    db.execute(
        "INSERT INTO search_blocks(content,section_id,source_start,source_end) VALUES(?1,?2,?3,?4)",
        params![text, section, start, end],
    )?;
    Ok(())
}
pub(crate) fn index(
    db: &Connection,
    workspace: i64,
    path: &str,
    hash: &str,
    rendered: &Rendered,
) -> Result<()> {
    remove(db, workspace, path)?;
    db.execute("INSERT INTO documents(workspace_id,path,title,revision) VALUES(?1,?2,?3,?4)
        ON CONFLICT(workspace_id,path) DO UPDATE SET title=excluded.title,revision=excluded.revision",params![workspace,path,rendered.title,hash])?;
    metadata(db, workspace, path, false)?;
    db.execute(
        "DELETE FROM document_blocks WHERE workspace_id=?1 AND path=?2",
        params![workspace, path],
    )?;
    for b in &rendered.blocks {
        db.execute(
            "INSERT OR REPLACE INTO document_blocks VALUES(?1,?2,?3,?4)",
            params![workspace, path, b.start as i64, b.text],
        )?;
    }
    let mut boundaries = vec![0];
    boundaries.extend(rendered.toc.iter().map(|h| h.source_start));
    boundaries.sort_unstable();
    boundaries.dedup();
    let mut hierarchy = Vec::new();
    for (i, start) in boundaries.iter().enumerate() {
        if let Some(h) = rendered.toc.iter().find(|h| h.source_start == *start) {
            while hierarchy
                .last()
                .is_some_and(|p: &&crate::TocEntry| p.level >= h.level)
            {
                hierarchy.pop();
            }
            hierarchy.push(h);
        }
        let end = boundaries.get(i + 1).copied().unwrap_or_else(|| {
            rendered
                .blocks
                .iter()
                .map(|b| b.end)
                .max()
                .unwrap_or(*start)
        });
        let blocks: Vec<_> = rendered
            .blocks
            .iter()
            .filter(|b| b.start >= *start && (i + 1 == boundaries.len() || b.start < end))
            .collect();
        if blocks.is_empty() && hierarchy.is_empty() {
            continue;
        }
        let headings = hierarchy
            .iter()
            .map(|h| h.text.as_str())
            .collect::<Vec<_>>()
            .join(" / ");
        // Heading blocks belong to navigation, but are not duplicated in the body column.
        let content = blocks
            .iter()
            .filter(|b| !rendered.toc.iter().any(|h| h.source_start == b.start))
            .map(|b| b.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let id = section(
            db,
            workspace,
            path,
            &rendered.title,
            &headings,
            &content,
            *start as i64,
            Some(end as i64),
            &headings,
        )?;
        for b in blocks {
            block(db, id, b.start as i64, Some(b.end as i64), &b.text)?;
        }
    }
    let text = rendered
        .blocks
        .iter()
        .map(|b| b.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    add_terms(
        db,
        workspace,
        path,
        &format!("{path} {} {text}", rendered.title),
    )?;
    Ok(())
}

#[derive(Clone)]
struct Term {
    text: String,
    phrase: bool,
    prefix: bool,
    alternatives: Vec<String>,
}
fn parse(query: &str) -> Vec<Term> {
    if query.len() > 1024 {
        return Vec::new();
    }
    let mut result = Vec::new();
    let mut chars = query.trim().chars().peekable();
    while let Some(c) = chars.next() {
        if c.is_whitespace() {
            continue;
        }
        let phrase = c == '"';
        let mut text = String::new();
        if !phrase {
            text.push(c);
        }
        for next in chars.by_ref() {
            if (phrase && next == '"') || (!phrase && next.is_whitespace()) {
                break;
            }
            text.push(next);
        }
        let explicit = !phrase && text.ends_with('*');
        let tokens = words(&text);
        if phrase && !tokens.is_empty() {
            result.push(Term {
                text: tokens.join(" "),
                phrase: true,
                prefix: false,
                alternatives: Vec::new(),
            });
        } else {
            let count = tokens.len();
            for (i, text) in tokens.into_iter().enumerate() {
                result.push(Term {
                    text,
                    phrase: false,
                    prefix: explicit && i + 1 == count,
                    alternatives: Vec::new(),
                });
            }
        }
        if result.len() > 16 {
            return Vec::new();
        }
    }
    if let Some(last) = result.last_mut() {
        if !last.phrase {
            last.prefix = true;
        }
    }
    result
}
fn expression(terms: &[Term], join: &str) -> String {
    terms
        .iter()
        .map(|t| {
            let mut choices = vec![format!(
                "{}{}",
                quoted(&t.text),
                if t.prefix { "*" } else { "" }
            )];
            choices.extend(t.alternatives.iter().map(|s| quoted(s)));
            format!("({})", choices.join(" OR "))
        })
        .collect::<Vec<_>>()
        .join(join)
}
fn distance(a: &str, b: &str) -> usize {
    let a: Vec<_> = a.chars().collect();
    let b: Vec<_> = b.chars().collect();
    if a.len().abs_diff(b.len()) > 1 {
        return 3;
    }
    let mut previous: Vec<_> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut current = vec![i + 1; b.len() + 1];
        for (j, y) in b.iter().enumerate() {
            current[j + 1] = (current[j] + 1)
                .min(previous[j + 1] + 1)
                .min(previous[j] + usize::from(x != y));
        }
        previous = current;
    }
    previous[b.len()]
}
fn typo(a: &str, b: &str) -> bool {
    if distance(a, b) <= 1 {
        return true;
    }
    let a: Vec<_> = a.chars().collect();
    let b: Vec<_> = b.chars().collect();
    let differences: Vec<_> = a
        .iter()
        .zip(&b)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i)
        .collect();
    a.len() == b.len()
        && differences.len() == 2
        && differences[1] == differences[0] + 1
        && a[differences[0]] == b[differences[1]]
        && a[differences[1]] == b[differences[0]]
}
fn corrections(db: &Connection, term: &str) -> Result<Vec<String>> {
    if !(3..=64).contains(&term.chars().count()) {
        return Ok(Vec::new());
    }
    let sigs = signatures(term);
    let placeholders = std::iter::repeat_n("?", sigs.len())
        .collect::<Vec<_>>()
        .join(",");
    let mut stmt = db.prepare(&format!("SELECT DISTINCT s.term FROM search_signatures s WHERE signature IN ({placeholders})
        AND EXISTS(SELECT 1 FROM search_vocab v WHERE v.term=s.term) ORDER BY s.term LIMIT {CANDIDATES}"))?;
    let rows = stmt.query_map(rusqlite::params_from_iter(sigs), |r| r.get::<_, String>(0))?;
    let mut result = Vec::new();
    for row in rows {
        let word = row?;
        if word != term && typo(term, &word) {
            result.push(word);
        }
    }
    result.sort_by(|a, b| distance(term, a).cmp(&distance(term, b)).then(a.cmp(b)));
    result.truncate(8);
    Ok(result)
}

pub(crate) fn query(
    db: &Connection,
    query: &str,
    workspace: Option<i64>,
) -> Result<Vec<SearchHit>> {
    let mut terms = parse(query);
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut hits = run(db, query, workspace, &terms)?;
    if hits.is_empty() && terms.iter().any(|t| !t.phrase) {
        for term in &mut terms {
            if !term.phrase {
                term.alternatives = corrections(db, &term.text)?;
            }
        }
        hits = run(db, query, workspace, &terms)?;
    }
    Ok(hits)
}
fn run(
    db: &Connection,
    raw: &str,
    workspace: Option<i64>,
    terms: &[Term],
) -> Result<Vec<SearchHit>> {
    let expr = expression(terms, " AND ");
    let filename = fold(raw.trim().trim_matches('"'));
    // Materialize bm25 before window functions: FTS auxiliary functions require the MATCH cursor.
    let mut stmt = db.prepare("WITH candidates AS MATERIALIZED (
        SELECT f.rowid AS id,f.workspace_id,f.path,f.title,f.source_start,f.source_end,f.section_title,
        '' AS excerpt,
        bm25(section_fts,12.0,8.0,10.0,2.0,1.0) AS score,
        CASE WHEN d.filename=?3 OR d.stem=?3 THEN 0 ELSE 1 END AS exact_tier,
        min(16,(length(highlight(section_fts,0,char(1),char(2)))-length(f.headings))/2)
        + min(16,(length(highlight(section_fts,1,char(1),char(2)))-length(f.title))/2) AS heading_strength
        FROM section_fts f JOIN search_documents d ON d.workspace_id=f.workspace_id AND d.path=f.path
        WHERE section_fts MATCH ?1 AND (?2 IS NULL OR f.workspace_id=?2)
        ), ranked AS (
        SELECT *,row_number() OVER(PARTITION BY workspace_id,path ORDER BY exact_tier,heading_strength DESC,score,source_start,id) AS best FROM candidates)
        SELECT id,workspace_id,path,title,score,source_start,source_end,section_title,excerpt FROM ranked
        WHERE best=1 ORDER BY exact_tier,heading_strength DESC,score,path,workspace_id,source_start LIMIT 50")?;
    let rows = stmt.query_map(params![expr, workspace, filename], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            SearchHit {
                workspace_id: r.get(1)?,
                path: r.get(2)?,
                title: r.get(3)?,
                score: r.get(4)?,
                source_start: r.get::<_, i64>(5)? as usize,
                source_end: r.get::<_, Option<i64>>(6)?.map(|v| v as usize),
                section_title: r.get(7)?,
                snippet: r.get(8)?,
                matched_terms: Vec::new(),
            },
        ))
    })?;
    let mut hits = Vec::new();
    let nav_expr = expression(terms, " OR ");
    let mut nav = db.prepare("SELECT source_start,source_end,snippet(search_blocks,0,'','',' … ',40),content
        FROM search_blocks WHERE search_blocks MATCH ?1 ORDER BY bm25(search_blocks,1.0,0.0),source_start LIMIT 1")?;
    for row in rows {
        let (id, mut hit) = row?;
        let found = nav
            .query_row(
                [format!("content : ({nav_expr}) AND section_id : \"{id}\"")],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, Option<i64>>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, String>(3)?,
                    ))
                },
            )
            .optional()?;
        let evidence = if let Some((start, end, snippet, content)) = found {
            hit.source_start = start as usize;
            hit.source_end = end.map(|v| v as usize);
            hit.snippet = snippet;
            content
        } else {
            format!(
                "{} {} {} {}",
                hit.section_title, hit.title, hit.path, hit.snippet
            )
        };
        if hit.snippet.is_empty() {
            hit.snippet = hit.section_title.clone();
        }
        let normalized = fold(&format!(
            "{} {} {} {evidence}",
            hit.section_title, hit.title, hit.path
        ));
        let mut matched = BTreeSet::new();
        for term in terms {
            for word in words(&term.text)
                .into_iter()
                .chain(term.alternatives.iter().cloned())
            {
                if normalized.contains(&word) {
                    matched.insert(word);
                }
            }
        }
        hit.matched_terms = matched.into_iter().collect();
        hits.push(hit);
    }
    Ok(hits)
}
