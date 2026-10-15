// Deterministic core-contract HTML, not a replacement Markdown parser or sanitizer.
export function fixture(sections) {
  const toc = [], html = [], source = [];
  const escape = text => text.replaceAll('&', '&amp;').replaceAll('<', '&lt;').replaceAll('>', '&gt;');
  for (let i = 0; i < sections; i++) {
    const start = source.join('\n').length;
    toc.push({ level: 2, id: `section-${i}`, text: `Section ${i}`, source_start: start });
    source.push(`## Section ${i}`, '', 'Reader text with a [local link](short.md) and [web link](https://example.com).', '', '| Key | Value |', '| --- | --- |', `| ${i} | deterministic |`, '');
    const code = Array.from({ length: 24 }, (_, n) => `const value${n} = ${i + n}; // deterministic code`).join('\n');
    const language = i % 3 === 0 ? 'javascript' : i % 3 === 1 ? 'unknown' : '';
    source.push('```' + language, code, '```', '');
    html.push(`<div data-source-start="${start}"><h2 id="section-${i}">Section ${i}</h2></div><p>Reader text with a <a href="short.md">local link</a> and <a href="https://example.com">web link</a>.</p><table><thead><tr><th>Key</th><th>Value</th></tr></thead><tbody><tr><td>${i}</td><td>deterministic</td></tr></tbody></table><pre><code${language ? ` class="language-${language}"` : ''}>${escape(code)}</code></pre>`);
    if (i % 20 === 0) { source.push('![Diagram](diagram.png)', ''); html.push('<p><img data-local-src="diagram.png" alt="Diagram"></p>'); }
  }
  // One pathological block exercises the plaintext limit.
  const huge = Array.from({ length: 1200 }, (_, i) => `const huge${i} = ${i};`).join('\n');
  if (sections > 10) { source.push('```javascript', huge, '```'); html.push(`<pre><code class="language-javascript">${huge}</code></pre>`); }
  return { workspace_id: 1, path: sections > 10 ? 'long.md' : 'short.md', title: sections > 10 ? 'Long' : 'Short', revision: 'fixed-1', content: source.join('\n'), html: html.join('\n'), toc };
}
export const imageBytes = [...Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aZ1sAAAAASUVORK5CYII=', 'base64')];

// Optional export for manual native-WebView profiling. dist/ is already ignored.
if (process.argv[1] === import.meta.filename && process.argv[2]) {
  const { mkdir, writeFile } = await import('node:fs/promises');
  const { join } = await import('node:path');
  const directory = process.argv[2]; await mkdir(directory, { recursive: true });
  for (const sections of [4, 300]) {
    const doc = fixture(sections);
    await writeFile(join(directory, doc.path), doc.content);
    await writeFile(join(directory, doc.path + '.html'), doc.html);
  }
  await writeFile(join(directory, 'diagram.png'), new Uint8Array(imageBytes));
  console.log(`Exported deterministic corpus to ${directory}`);
}
