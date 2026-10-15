import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { waitFor } from '@testing-library/svelte';
import { api } from './api';
import { assetObjectUrl, enhanceLocalImages, imageDimensions } from './images';
import type { Asset, ResolvedLink } from './types';

vi.mock('./api', () => ({ api: { resolve: vi.fn(), asset: vi.fn() } }));
const asset: Asset = { path: 'assets/a.png', mime_type: 'image/png', bytes: [137, 80, 78, 71] };
let root: HTMLElement;
let stop: (() => void) | undefined;
let current: boolean;
function start(relative = '../assets/a.png') {
  const image = document.createElement('img');
  image.dataset.localSrc = relative;
  image.alt = 'Architecture diagram';
  root.append(image);
  stop = enhanceLocalImages(root, { workspace_id: 7, path: 'notes/readme.md' }, () => current);
  return image;
}
beforeEach(() => {
  vi.clearAllMocks(); current = true; stop = undefined;
  root = document.createElement('article'); document.body.append(root);
  vi.mocked(api.resolve).mockResolvedValue({ kind: 'asset', path: asset.path, fragment: null });
  vi.mocked(api.asset).mockResolvedValue(asset);
  vi.spyOn(URL, 'createObjectURL').mockReturnValue('blob:workspace-image');
  vi.spyOn(URL, 'revokeObjectURL').mockImplementation(() => {});
});
afterEach(() => { stop?.(); root.remove(); });

describe('local image enhancement', () => {
  it('reserves PNG header dimensions before reveal and restores attributes on cleanup', async () => {
    const bytes = new Uint8Array(24), view = new DataView(bytes.buffer);
    view.setUint32(0, 0x89504e47); view.setUint32(12, 0x49484452); view.setUint32(16, 640); view.setUint32(20, 480);
    const png = { ...asset, bytes: [...bytes] };
    expect(imageDimensions(png)).toEqual({ width: 640, height: 480 });
    vi.mocked(api.asset).mockResolvedValue(png);
    const image = start(); await waitFor(() => expect(image.getAttribute('src')).toBe('blob:workspace-image'));
    expect(image.width).toBe(640); expect(image.height).toBe(480);
    stop!(); expect(image.hasAttribute('width')).toBe(false); expect(image.hasAttribute('height')).toBe(false);
    expect(imageDimensions(asset)).toBeUndefined();
    view.setUint32(16, 0xffffffff); expect(imageDimensions({ ...png, bytes: [...bytes] })).toBeUndefined();
  });
  it('resolves relative paths against the document, assigns a typed blob, and revokes it on cleanup', async () => {
    const image = start();
    expect(image.hasAttribute('src')).toBe(false);
    await waitFor(() => expect(image.getAttribute('src')).toBe('blob:workspace-image'));
    expect(api.resolve).toHaveBeenCalledWith(7, 'notes/readme.md', '../assets/a.png');
    expect(api.asset).toHaveBeenCalledWith(7, 'assets/a.png');
    const blob = vi.mocked(URL.createObjectURL).mock.calls[0][0] as Blob;
    expect(blob.type).toBe('image/png'); expect(blob.size).toBe(asset.bytes.length);
    image.dispatchEvent(new Event('load'));
    expect(image.hasAttribute('aria-busy')).toBe(false);
    stop!();
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:workspace-image');
    expect(image.hasAttribute('src')).toBe(false);
    expect(image.alt).toBe('Architecture diagram');
  });
  it.each(['https://tracker.invalid/a.png', '//tracker.invalid/a.png', 'data:image/png;base64,a', 'file:///a.png', 'javascript:alert(1)', '/absolute.png', '\\server\\image.png', '#fragment', ''])('rejects non-relative image references: %s', async relative => {
    const image = start(relative);
    await waitFor(() => expect(root.querySelector('.image-fallback')).not.toBeNull());
    expect(api.resolve).not.toHaveBeenCalled(); expect(api.asset).not.toHaveBeenCalled();
    expect(image.hasAttribute('src')).toBe(false); expect(URL.createObjectURL).not.toHaveBeenCalled();
  });
  it.each<ResolvedLink>([{ kind: 'external', url: 'https://tracker.invalid/a.png' }, { kind: 'document', path: 'readme.md', fragment: null }])('requires an asset resolution: $kind', async result => {
    vi.mocked(api.resolve).mockResolvedValue(result);
    start();
    await waitFor(() => expect(root.querySelector('.image-fallback')?.textContent).toContain('local asset'));
    expect(api.asset).not.toHaveBeenCalled(); expect(URL.createObjectURL).not.toHaveBeenCalled();
  });
  it('shows accessible alt text for backend failures without injecting error HTML', async () => {
    vi.mocked(api.asset).mockRejectedValue({ message: '<script>Missing image</script>' });
    const image = start();
    await waitFor(() => expect(root.querySelector('[role="img"]')?.getAttribute('aria-label')).toContain('Architecture diagram — image unavailable'));
    expect(root.querySelector('.image-fallback')?.textContent).toContain('<script>Missing image</script>');
    expect(root.querySelector('script')).toBeNull(); expect(image.hidden).toBe(true);
    expect(image.hasAttribute('src')).toBe(false);
  });
  it('rejects SVG and other unsupported MIME types', async () => {
    vi.mocked(api.asset).mockResolvedValue({ ...asset, mime_type: 'image/svg+xml' });
    start();
    await waitFor(() => expect(root.querySelector('.image-fallback')?.textContent).toContain('Unsupported image type'));
    expect(URL.createObjectURL).not.toHaveBeenCalled();
  });
  it('revokes an undecodable image and provides an accessible fallback', async () => {
    const image = start();
    await waitFor(() => expect(image.getAttribute('src')).toBe('blob:workspace-image'));
    image.dispatchEvent(new Event('error'));
    expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:workspace-image');
    expect(root.querySelector('[role="img"]')?.getAttribute('aria-label')).toContain('could not be decoded');
    expect(image.hasAttribute('src')).toBe(false);
  });
  it('does not fetch bytes if resolution completes after cleanup', async () => {
    let resolve!: (result: ResolvedLink) => void;
    vi.mocked(api.resolve).mockImplementation(() => new Promise(done => { resolve = done; }));
    start(); stop!();
    resolve({ kind: 'asset', path: asset.path, fragment: null });
    await Promise.resolve();
    expect(api.asset).not.toHaveBeenCalled(); expect(URL.createObjectURL).not.toHaveBeenCalled();
  });
  it.each(['cleanup', 'document switch', 'detached image'])('ignores asset bytes completing after %s', async reason => {
    let resolve!: (result: Asset) => void;
    vi.mocked(api.asset).mockImplementation(() => new Promise(done => { resolve = done; }));
    const image = start();
    await waitFor(() => expect(api.asset).toHaveBeenCalled());
    if (reason === 'cleanup') stop!();
    else if (reason === 'document switch') current = false;
    else image.remove();
    resolve(asset); await Promise.resolve();
    expect(URL.createObjectURL).not.toHaveBeenCalled(); expect(image.hasAttribute('src')).toBe(false);
    expect(root.querySelector('.image-fallback')).toBeNull();
  });
});
it('creates only allowlisted raster asset URLs', () => {
  expect(() => assetObjectUrl({ ...asset, mime_type: 'text/html' })).toThrow('Unsupported image type');
  expect(URL.createObjectURL).not.toHaveBeenCalled();
});
