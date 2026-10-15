# Reader performance

## Reproduce without new dependencies

Requires the installed project dependencies, Node 26 (native `WebSocket`) and `/usr/bin/chromium`.

```sh
node scripts/performance/run.mjs after /tmp/mdwell-after.json
```

The tool owns a Vite subprocess on port 1420 and an isolated headless Chromium subprocess. It has a 120-second watchdog, bounded startup/CDP requests, and cleanup for the browser, Vite, and temporary profile/corpus. Port conflicts fail instead of attaching to or terminating an existing server. Do not run other builds/tests concurrently when comparing timings. It uses the installed project dependencies and requires no additional packages.

The real `src/App.svelte` is served by Vite; the only API substitute is `window.__TAURI_INTERNALS__.invoke`. The corpus generator produces deterministic Markdown and **core-contract-like safe HTML**, not HTML actually rendered/sanitized by Rust. All text/markup is generated from controlled constants and numbers. This measures frontend work, not Rust render time, disk scans, IPC serialization, or actual native WebView performance. An avoided `read_document` call also avoids a backend render, but that backend saving is not timed here.

Corpus:

- Short: 142 lines, four headings/code blocks, one local image.
- Long: 11,732 lines, 300 headings, 301 code blocks, 300 tables, local/web links, 15 local images.
- Every third block has an explicit JavaScript language; the rest are unknown/unlabelled. The long corpus adds a 1,200-line JavaScript block to exercise the plaintext cutoff.
- 1,001 library summaries exercise the sidebar separately from the reader.
- The deterministic images are tiny 1×1 PNGs. This checks loading/cleanup/invalidation, **not** the layout-shift behavior of large diagrams or slow disk reads.

Generated fixtures live in a temporary directory and are deleted automatically. For native profiling, export them into the already-ignored `dist/` directory (no massive fixtures or `.gitignore` changes):

```sh
node scripts/performance/fixtures.mjs dist/performance-corpus
```

The scroll sweep sets `scrollTop` over 240 RAF callbacks, down and back up, after a six-second settle period that includes an unchanged four-second poll. Geometry/selector probes are categorized as scroll-listener, application RAF, or other work. The driver uses an unwrapped RAF and does not read layout inside the sweep. Highlight calls are timed by a harness-only Vite transform. Long tasks and layout shifts use `PerformanceObserver`; frame p95 is the interval between driver RAF timestamps, **not FPS or compositor latency**. Listener timings include probe overhead and are close to the clock's resolution after optimization.

The harness also asserts active-heading correctness after top/end/arbitrary jumps and changing the real reading-size setting to 24px and back to 18px. These assertions run after the measured sweep and are excluded from its metrics.

## Measured baseline and after

Measured on this Linux environment with Node v26.11.1, Chromium 154.0.8037.97, a requested 1440×1000 window, Vite development build, no CPU throttling. Baseline was captured **before frontend changes**. Values are individual samples, not statistically controlled multi-run medians; startup/dependency-cache noise is especially visible on the first short scenario. Raw JSON was written to `/tmp/mdwell-baseline.json` and `/tmp/mdwell-after.json` when these samples were captured; these local files are not repository artifacts.

| Metric | Short baseline | Short after | Long baseline | Long after |
| --- | ---: | ---: | ---: | ---: |
| Scroll-listener geometry reads | 1,190 | 0 | 71,638 | 0 |
| Scroll-listener selector queries | 238 | 0 | 238 | 0 |
| Application RAF geometry reads during sweep | 0 | 0 | 0 | 0 |
| Scroll listener p95, ms | 0.2 | 0.1 | 3.4 | 0.1 |
| Heading RAF work p95, ms | — | 0.1 | — | 0.1 |
| Frame interval p95, ms | 16.8 | 16.7 | 16.7 | 16.8 |
| Scroll-window long tasks ≥50ms | 0 | 0 | 0 | 0 |
| Timed highlight calls | 4 | 2 | 301 | 100 |
| Highlight CPU total, ms | 63.0 | 6.0 | 685.5 | 36.2 |
| Highlight call p95, ms | 52.3 | 5.4 | 3.9 | 0.6 |
| Load long-task durations, ms | 245 | 196 | 954, 76 | 123 |
| `read_document` calls after two unchanged polls | 3 | 1 | 3 | 1 |
| Settled article height, px | 2,771.296875 | same | 225,473.453125 | same |
| Observed load layout-shift sum | 0 | 0 | 0 | 0 |

The main demonstrated improvement is removal of heading-count-dependent scroll work and synchronous all-block/auto-detection highlighting. The long load still has a substantial task from mounting/parsing/layout/library work; this is not a claim that all load work is solved. Frame p95 was already about one 60Hz interval before changes and remains so. **No FPS improvement is claimed.** Lower total highlight time partly reflects the intentional policy change: unknown/unlabelled and oversized blocks remain plaintext.

## Post-search integration measurement

The harness now also opens the real search palette, selects a mocked core-contract `SearchHit` in the already-open document, asserts one text walk confined to its heading block, and repeats the 240-frame sweep with search marks present. The fixture/corpus and initial sweep are unchanged. This is frontend navigation/marking coverage, not a measurement of backend search ranking. The heading-only browser hit is intentionally small; unit/UI tests additionally cover multi-block/code/link/Unicode matches.

Recorded with the same Node/Chromium setup; JSON is `/tmp/mdwell-post-search.json`:

| Metric | Short post-search | Long post-search |
| --- | ---: | ---: |
| Initial scroll handler p95, ms | 0.1 | 0.1 |
| Initial frame interval p95, ms | 16.8 | 16.7 |
| Highlight calls / total CPU, ms | 2 / 5.1 | 100 / 30.0 |
| Load long-task durations, ms | 178 | 115 |
| Text walks selecting hit | 1 selected block | 1 selected block |
| Document reads after same-document hit and three unchanged polls | 1 | 1 |
| Marked sweep listener invocations | 239 | 239 |
| Marked sweep scroll + application RAF geometry reads | 0 | 0 |
| Marked sweep scroll + application RAF selector queries | 0 | 0 |
| Marked sweep handler / RAF work p95, ms | 0.1 / 0.0 | 0.1 / 0.1 |
| Marked sweep frame interval p95, ms | 16.8 | 16.8 |
| Marked sweep long tasks | 0 | 0 |

The original sweep has 238 events because it starts at scroll position zero; the marked sweep has 239 because the selected hit starts mid-document. Article heights remain identical to the original baseline and observed load layout-shift sums remain zero. The 0.0ms RAF measurement is below the probe's clock resolution, not literally zero CPU work. No settled-scroll regression was observed in this sample; startup timings remain noisy and native WebView performance is still unmeasured. These numbers do not establish an FPS gain.

## Chromium heap and DOM snapshots

The harness records `Runtime.getHeapUsage` and `Memory.getDOMCounters` in each scenario's `load.memory` and `search.memory` fields. `memoryMethodology` identifies the CDP methods and records `forcedGC: false`. Heap values are bytes; DOM values are counts. The two CDP calls are adjacent snapshots, not an atomic measurement.

- **Settled initial:** after the existing six-second settle interval, before the initial scroll sweep.
- **After search:** after selecting the hit and waiting for the existing three-RAF settle, before the marked scroll sweep. This checkpoint also follows the initial sweep and reading-preference assertions, so its difference is not attributable solely to search.
- No forced garbage collection is performed. Chromium uses its normal GC schedule; scenarios reuse the same browser target. Uncollected or retained documents/nodes, Vite development modules, fixture data, and harness instrumentation can contribute to the values.

Actual standalone sample, Node v26.11.1 / Chromium 154.0.8037.97, same corpus and requested window size, with no concurrent tests or builds:

```sh
node scripts/performance/run.mjs memory-sample /tmp/mdwell-memory-sample.json
```

| CDP metric | Short initial | Short after search | Long initial | Long after search |
| --- | ---: | ---: | ---: | ---: |
| JS heap used (`usedSize`), bytes | 14,819,016 | 15,521,948 | 31,718,484 | 15,792,808 |
| JS heap allocated (`totalSize`), bytes | 20,717,568 | 21,241,856 | 49,926,144 | 45,211,648 |
| Embedder heap used, bytes | 14,581,264 | 18,183,872 | 57,116,456 | 86,493,352 |
| Backing storage, bytes | 3,247,011 | 3,261,743 | 4,012,742 | 4,026,786 |
| DOM nodes | 8,413 | 8,727 | 39,044 | 39,145 |
| Documents | 5 | 5 | 2 | 2 |
| JS event listeners | 32 | 38 | 356 | 360 |

These are **Chromium target heap/DOM counters**, not browser process RSS, total application memory, or native GTK/WebKit/Tauri process memory. DOM counters are not restricted to the visible article and do not by themselves prove a leak. Do not sum the heap categories into a process-memory estimate. Ordinary GC/allocation timing can change snapshots substantially; the lower long-scenario used heap after search is not evidence of a search optimization. **Memory baseline measurements are unavailable, so no memory savings are claimed.**

This run also retained zero scroll/application-RAF geometry reads and selector queries, with no scroll-window long tasks, in both initial and marked sweeps.

## Implementation and invariants

- `src/reader-scroll.ts`: collect heading elements once per rendered document; cache offsets in reader scroll coordinates; passive scroll listener coalesces into one RAF; binary search selects the last heading at or above the existing 80px threshold. Update Svelte state only when the selected ID changes. No selectors/geometry reads in the scroll listener or selection RAF.
- Offset invalidation is independent of scrolling: article/reader `ResizeObserver`, captured image load/error, window resize, font readiness/loading completion, and explicit reading-size/typeface/sidebar changes. A rebuild is O(headings), but batched outside ordinary scroll selection. Resize while images load can legitimately produce geometry reads during an interaction; the zero-read measurement is for a settled document.
- `src/reader-highlighting.ts`: defer the first slice, highlight only explicit registered languages, retain plaintext above 16,000 characters or 400 lines, yield after roughly 4ms per slice, and defer pending slices while scrolling and for 150ms afterwards. A single highlight call is indivisible: the 4ms budget is a yield target, not a hard deadline. No `highlightAuto` call. Existing find/search marks are preserved, copy consumes the original text, and jobs/copy feedback/listeners are cleaned up. Once the queue finishes, its scroll listener is removed.
- `src/style.css`: code uses the same block geometry before and after token insertion; token spans inherit code font metrics so highlighting does not introduce bold/italic width changes. No content-visibility, virtualization, cosmetic reader layout redesign, or editor changes.
- `src/App.svelte`: summary fields are compared before assigning lists; a separate cached tree is rebuilt only for changed summaries. Polls still refresh every workspace and list summaries, but `api.read` runs only if the current file's indexed revision differs. Navigation, dirty-draft, and save guards are checked before/after the read. A missing summary does not trigger repeated reads of a removed file. Polling does not sync remotely.
- Enhancement helper setup uses `untrack` so synchronous identity callbacks do not accidentally subscribe effects to unrelated reader state. Explicit document/HTML and mode dependencies remain.
- `src/images.ts`: safe PNG/GIF/JPEG headers reserve intrinsic width/height, when neither attribute was supplied, before assigning the typed local blob URL. Cleanup restores original attributes. Unknown/truncated formats, WebP/BMP, and JPEG orientation details still rely on decode/load. Geometry is not known before backend bytes arrive; a later arrival/fallback can move content. Native browser scroll anchoring remains enabled, and cached heading offsets are invalidated after image changes. Zero observed CLS in this small-image corpus is not a general no-shift guarantee.

- `src/reader-search.ts`: NFD/accent/case folding matches the core while mapping matches back to exact UTF-16 ranges, including decomposed accents and astral characters. Prefixes and vocabulary-corrected terms come from `matched_terms`; the frontend does not invent typo corrections. Result context/snippets render escaped text segments. Reader text walks are restricted to source-overlapping blocks (exclusive `source_end`), with a nearest-block legacy fallback when the end is unavailable. The block lookup itself enumerates source wrappers once per navigation, not per scroll. Marks wrap text nodes across inline/token spans without replacing links and exclude buttons/hidden content. Explicit ranges with no overlap are not highlighted; scrolling can still fall back to the nearest block. Visible marks are preferred for scrolling.
- Search and document-find marks are distinct layers. Clearing unwraps children rather than replacing `textContent`, preserving the other layer and links; only affected parents are normalized. Search locations carry ranges/terms in navigation history. Source still receives the original byte offset through `editor.goTo`. Document find waits 150ms after the latest input, cancels stale document/mode work, and flushes on Enter/previous/next without a second delayed walk.

Tests cover binary-search boundaries/jumps, coalescing without geometry reads, resize/load invalidation, teardown, known-language/size policy, deferred/cancelled highlighting, scrolling deferral, copy cleanup, find-mark preservation, unchanged/revision-changed polling, draft safety, image dimensions, SSH URL submission and Git error guidance, safe result context/snippets, Unicode/prefix/corrected-term marking, scoped multi-block/code navigation, preserved links/copy/find layers, repeated hits/history, stale navigation/find cancellation, and unchanged Source offsets. The post-search frontend suite has 71 passing tests.

## Native GTK/WebKit verification (manual; not yet measured)

Chromium is not the Linux Tauri GTK/WebKit WebView. Do not extrapolate browser timings into native frame rates.

1. Export the corpus above, run the normal desktop development workflow, and open the generated directory as a local workspace. Use the Markdown files so the **real Rust renderer/indexer** participates. Do not import `.html` as trusted reader content.
2. In a debug Tauri build, open WebKit developer tools using the platform's inspector action/shortcut (commonly Ctrl+Shift+I). If disabled, enable developer tools in your local debug configuration.
3. Record the inspector Timeline around opening `long.md`, several continuous scrolls and TOC jumps, and at least two unchanged four-second polling intervals. Inspect JavaScript, layout, paint and long tasks separately. Check native disk/IPC/render timing with the backend profiler if necessary.
4. Repeat with the sidebar shown/hidden, 14px/24px text, serif/sans fonts, a smaller window, delayed large local images and broken images. Verify the active heading follows jumps after each layout change; heading selection must not scan DOM geometry on each scroll.
5. Switch to Source and back; navigate away during pending enhancements; type while a changed revision is loading; verify drafts and saved scroll positions remain intact. Copy short/plaintext/large blocks and test find while highlighting is pending.
6. Watch unchanged polling: summary scans may run, but `read_document`/Rust rendering should not. Change a file externally and verify one changed-revision read updates a clean reader without replacing a dirty draft.
7. Save native traces and report actual frame/task distributions with machine/WebKit/build details. Prefer repeated runs on the same machine and corpus; avoid speculative FPS estimates.

## Related documentation

See [GitHub authentication](AUTHENTICATION.md) for accepted HTTPS/SSH repository URLs, verified SSH host keys, and Git credential-helper setup. Repository URL drafts are not persisted. Vellistry's integrated [GitHub Device Flow](GITHUB_INTEGRATION.md) stores tokens only in the native credential store, never in WebView preferences or plaintext files. Existing `mdwell` benchmark paths remain unchanged.
