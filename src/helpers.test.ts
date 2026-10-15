import { describe, expect, it } from 'vitest';
import { byteToPosition, fileTree, isWebUrl, isConflict, errorMessage, sameSummaries } from './helpers';

describe('source navigation', () => {
  it('converts UTF-8 offsets to UTF-16 positions', () => {
    const text = 'aé😀z';
    expect([0, 1, 2, 3, 7, 8, 99].map(n => byteToPosition(text, n))).toEqual([0, 1, 1, 2, 4, 5, 5]);
  });
});
describe('file tree', () => {
  it('compares all summary fields before preserving tree identity', () => {
    const doc = { workspace_id: 1, path: 'a.md', title: 'A', revision: 'r1' };
    expect(sameSummaries([doc], [{ ...doc }])).toBe(true);
    expect(sameSummaries(undefined, [])).toBe(false);
    for (const change of [{ revision: 'r2' }, { title: 'B' }, { path: 'b.md' }, { workspace_id: 2 }]) expect(sameSummaries([doc], [{ ...doc, ...change }])).toBe(false);
    expect(sameSummaries([doc], [])).toBe(false);
  });
  it('keeps nested paths distinct and sorts folders first', () => {
    const docs = ['z.md', 'notes/b.md', 'notes/a.md', 'other/a.md'].map(path => ({ workspace_id: 1, path, title: path, revision: 'r' }));
    const tree = fileTree(docs);
    expect(tree.map(n => n.name)).toEqual(['notes', 'other', 'z.md']);
    expect(tree[0].children.map(n => n.path)).toEqual(['notes/a.md', 'notes/b.md']);
  });
});
describe('security and transport', () => {
  it('permits only explicit web URLs', () => {
    expect(isWebUrl('https://example.com')).toBe(true);
    for (const url of ['javascript:alert(1)', 'file:///etc/passwd', 'data:text/html,x', '//example.com', 'notes.md']) expect(isWebUrl(url)).toBe(false);
  });
  it('recognizes structured conflicts without treating generic errors as conflicts', () => {
    expect(isConflict({ code: 'conflict', message: 'changed' })).toBe(true);
    expect(isConflict(new Error('conflict'))).toBe(false);
    expect(errorMessage({ code: 'io', message: 'Permission denied' })).toBe('Permission denied');
  });
});
