# Search backend

## Inspection and measured baseline (before edits)

The original `lib.rs` pipeline stores one `document_fts` row per document,
concatenating all headings and all rendered blocks. Literal queries use AND of
exact tokens (prefixes require explicit `*`). BM25 weights are headings 12, title
10, filename 8, path 6, body 1; there is no deterministic exact filename/stem
tier. Every hit runs a separate complete `document_blocks` scan for navigation.
If there are no hits, a single-word typo scans every document filename stem;
body/heading typos and independent multiword corrections are unsupported.

Before changing production code, ran:

```sh
cargo run -p mdwell-core --release --example search_quality
```

On the 15-query English/German corpus in that example: top-1 **0.667**, MRR
**0.667**, mean precision@50 **0.500** (one relevant document per query; precision
uses the actual returned count, empty results score zero). Failed queries:
`oauth oid`, `oatuh odic`, `doker compsoe`, `Zugriffsschlüssel`, `troubleshoot`.
1,000 added synthetic documents: index 137.7 ms, search Docker p50 2,123 µs / p95
2,377 µs (100 repetitions), one-document refresh 9.0 ms, checkpointed database
1,413,120 bytes. These are local measurements, not portable latency promises.

## Migration plan

Versioned transactional migration builds sections from existing cached
`document_fts` and `document_blocks`, without filesystem or network reads. Old
heading text is retained in a single fallback section because the old cache
cannot reliably recover heading levels, hierarchy, or end offsets. Cached block
starts remain navigable; their unknown source ends remain null. Workspace and
document metadata, including revision hashes, are preserved. Separate rebuild
markers bypass the unchanged-revision shortcut on the next successful refresh;
offline roots do not prevent migration or immediate search. Failure rolls back
migration; a database from a newer search schema is rejected rather than reset.

The concrete version is `search_schema.version = 1` (search-owned, not SQLite's
application-wide `user_version`). Migration takes an immediate transaction,
creates the new tables and both FTS indexes, copies every document, and only then
drops `document_fts` and commits the version. Concurrent openers serialize on
SQLite. `search_documents.rebuild` is cleared only by successful indexing in the
refresh/save transaction. Search never calls Git or accesses workspace roots.
Legacy heading matches cannot provide reconstructed heading offsets: their
fallback section uses the first cached block start and a null end until refresh.

## Index and ranking

`section_fts` contains one row for the preamble (if present) and each heading's
following blocks up to the next heading. A stack of heading levels produces
inherited paths such as `Authentication / OAuth OIDC / Refresh tokens`. A child
inherits its ancestor headings, not their body text. Heading text is indexed in
the heading field, not copied into each body block. Body text is flattened from
Markdown parser events; inline emphasis/link boundaries do not split words.

`search_blocks` stores rendered block text, the section identity, and original
UTF-8 source start/end bytes. Navigation intersects a content FTS query with an
indexed section-identity term, selects the best matching block, and asks FTS for
a nearby plain-text excerpt. It does **not** scan the document's block list for
each hit. The redundant legacy `document_blocks` cache remains maintained for
compatibility; it is not used by normal search.

Ranking is lexicographic:

1. Accent/case-folded exact filename or stem equal to the whole query (outer
   quotes ignored). These matches always precede non-exact documents, regardless
   of term frequency. An explicit prefix is not an exact filename lookup.
2. Matched heading/title strength (FTS highlight occurrence counts, individually
   capped at 16); body-only incidental mentions lose to heading matches.
3. Weighted section BM25: headings **12**, filename **10**, title **8**, path
   **2**, body **1**. Scores are not comparable across unrelated queries.
4. Stable path, workspace identity, source start, and section row identity ties.

A SQL window chooses each document's best section **before** the 50-document
limit. One document with many matching headings cannot crowd out the other 49.
The returned `score` is raw BM25, not a numeric encoding of the ranking tiers;
consumers should retain backend order rather than resort by score.

## Query rules and bounds

- Unquoted text is tokenized into Unicode alphanumeric words and ANDed. Thus
  `CI/CD` becomes `ci AND cd*`; punctuation is not FTS operator syntax.
- Only the final unquoted token has an implicit prefix; explicit `word*` also
  works for earlier terms. `OAuth oid` finds OIDC without modifying OAuth.
- Quoted text is a contiguous token phrase, including phrases spanning inline
  formatting. Phrases are never prefix-expanded or typo-corrected. `"README"`
  remains an exact token phrase; filename ranking still applies.
- AND is section-local: terms in unrelated sibling sections do not combine.
  Different blocks of the same section can combine, and inherited headings can
  combine with descendant body text.
- If the initial query has no hits, each unquoted word independently gets up to
  eight alternatives, ORed **within** its original AND slot. Phrases stay exact.
  Known technical words are retained, not replaced. No OR across query slots.
- Candidate lookup uses persisted one-character deletion signatures with a
  `(signature, term)` primary-key index and validates against SQLite's current
  `fts5vocab`. At most **128 candidates** per query word are distance-checked;
  correction words must be **3–64 characters**. Accepted changes are edit
  distance **one**, or **one adjacent transposition** (Levenshtein distance two).
  Arbitrary distance-two rewrites are not accepted. Removed vocabulary signatures
  are pruned when their last document is reindexed/deleted.
- Queries over **1,024 bytes** or **16 token/phrase slots** return no results,
  rather than silently discarding AND terms. FTS operators are always quoted as
  literal words; no embeddings, external executable, or new dependency is used.

German compounds work when the query is a compound **prefix**, e.g.
`Zugriffsschlüssel` → `Zugriffsschlüsselverwaltung`. There is no compound
splitting, stemming, translation, ß-to-ss transliteration, semantic search, or
suffix/substring matching. Optional trigram substring retrieval is deliberately
not enabled; `schlüsselverwaltung` alone does not find that compound. Accents
and case are normalized consistently with the `unicode61 remove_diacritics 2`
index for the Latin corpus. This is not a full language-aware tokenizer.

## SearchHit transport

Existing fields are retained: `workspace_id`, `path`, `title`, `score`,
`source_start`, `snippet`. Added fields (serde-defaulted for older payloads):

- `section_title: String`: inherited heading path (empty for a preamble).
- `source_end: Option<usize>`: matching block's exclusive UTF-8 source byte end;
  null for a legacy migrated cache.
- `matched_terms: Vec<String>`: sorted normalized query words/corrections found
  in selected-block/heading/title/path evidence. Prefix terms can be partial
  words; they are not necessarily the original user spelling after correction.

Snippets are plain text, with no HTML or marker delimiters. No highlight spans
are returned. A frontend can accent/case-fold the snippet and terms to highlight
matches, but must map folded positions back to the original Unicode boundaries.
The frontend renders escaped text segments, carries the selected source range into navigation history, and marks only overlapping source blocks. It scrolls to the first visible matched text where available, falling back to the source block; links and document-find marks remain intact.

`source_start` and `source_end` identify a **rendered block**, not the exact
matched character. Inline markup, entity decoding, and flattened list/table/code
text do not admit a reliable character-to-source mapping from this cache. In a
long code block the snippet is near the match, but the backend offset still points to
the fence/block start. The reader resolves matched terms inside that block and scrolls to a visible mark when possible. A heading-only inherited match with no local matching
block uses its section's range; filename-only matches also use the section
fallback. Do not present these offsets as exact token ranges.

## Reproducible quality and performance

`tests/search_corpus.rs` is shared by the regression suite and the example. It
contains eight Markdown documents and 15 judged queries covering English and
German, OAuth/OIDC, Docker Compose, GitHub Actions, CI/CD segmentation, README
filename/quoted lookup, accents/case, compound prefixes, adjacent transpositions,
independent multiword typos, inherited headings, inline phrases, and repeated
incidental body noise. Each query declares exactly one relevant document.
These are small deterministic fixtures, not a claim about general IR accuracy.

The example prints a frozen, corpus-specific legacy SQL quality snapshot and
then current results; the snapshot reproduces the measured pre-edit quality
numbers. It is not a second production search implementation. None of these
queries produces a legacy filename-distance fallback hit. Separate regressions
cover 70 matching multi-section documents (dedup and 50-limit), long code blocks,
UTF-8/frontmatter offsets, AND/phrase non-expansion, index cleanup, offline legacy
migration, preserved revisions, rebuild markers, transactional rollback, and
future-version rejection.

```sh
cargo test -p mdwell-core
cargo clippy -p mdwell-core --all-targets -- -D warnings
cargo run -p mdwell-core --release --example search_quality
```

Measured final run on the same local environment:

| Metric | Original | Section backend |
| --- | ---: | ---: |
| Top-1 accuracy | 0.667 | 1.000 |
| MRR | 0.667 | 1.000 |
| Mean precision@50 (actual returned count) | 0.500 | 0.700 |
| Add 1,000 three-heading documents | 137.7 ms | 956.5 ms |
| Docker search p50 / p95 | 2.123 / 2.377 ms | 6.514 / 7.303 ms |
| One changed document refresh | 9.0 ms | 17.6 ms |
| Checkpointed SQLite bytes | 1,413,120 | 4,399,104 |

Additional section-backend search p50 / p95: inherited `OAuth refresh`
0.147 / 0.356 ms; multiword typo `doker compsoe` 0.511 / 0.851 ms; compound
prefix 0.124 / 0.180 ms; inline phrase 0.130 / 0.163 ms. Each timing uses 100
warm repeated queries after adding 1,000 files to the eight-document corpus.
The incremental refresh reports exactly one indexed and 1,007 unchanged files.
Database bytes include document metadata, both FTS indexes, cached blocks,
vocabulary ownership, and deletion signatures after a WAL checkpoint.

**Tradeoff:** richer section ranking and indexed typo vocabulary cost indexing
time, storage (~3.1×), and high-fanout search latency versus the simpler original.
The indexed navigation improvement avoids per-hit full block scans, but does not
eliminate FTS ranking work for every matching section. There is no performance
claim based solely on improved relevance; rerun the harness on representative
workspaces before establishing latency or size budgets.
