import { api } from './api';
import { errorMessage } from './helpers';
import type { Asset } from './types';

export function assetObjectUrl(asset: Asset): string {
  if (!['image/png', 'image/jpeg', 'image/gif', 'image/webp', 'image/bmp'].includes(asset.mime_type)) {
    throw new Error('Unsupported image type.');
  }
  return URL.createObjectURL(new Blob([new Uint8Array(asset.bytes)], { type: asset.mime_type }));
}

// Reserve intrinsic geometry from available bytes before assigning the blob URL.
// Unknown formats still rely on decode/load and the reader's resize invalidation.
export function imageDimensions(asset: Asset): { width: number; height: number } | undefined {
  const bytes = new Uint8Array(asset.bytes), view = new DataView(bytes.buffer);
  let width = 0, height = 0;
  if (asset.mime_type === 'image/png' && bytes.length >= 24 && view.getUint32(0) === 0x89504e47 && view.getUint32(12) === 0x49484452) {
    width = view.getUint32(16); height = view.getUint32(20);
  } else if (asset.mime_type === 'image/gif' && bytes.length >= 10 && String.fromCharCode(...bytes.slice(0, 3)) === 'GIF') {
    width = view.getUint16(6, true); height = view.getUint16(8, true);
  } else if (asset.mime_type === 'image/jpeg' && bytes[0] === 0xff && bytes[1] === 0xd8) {
    let offset = 2;
    while (offset + 8 < bytes.length && bytes[offset] === 0xff) {
      const marker = bytes[offset + 1], length = view.getUint16(offset + 2);
      if (length < 2 || offset + 2 + length > bytes.length) break;
      if ([0xc0, 0xc1, 0xc2, 0xc3, 0xc5, 0xc6, 0xc7, 0xc9, 0xca, 0xcb, 0xcd, 0xce, 0xcf].includes(marker)) {
        height = view.getUint16(offset + 5); width = view.getUint16(offset + 7); break;
      }
      offset += 2 + length;
    }
  }
  return width > 0 && height > 0 && width <= 32768 && height <= 32768 ? { width, height } : undefined;
}

// Inert core image references are capabilities only after the backend resolves them.
export function enhanceLocalImages(
  root: HTMLElement,
  document: { workspace_id: number; path: string },
  isCurrent: () => boolean,
): () => void {
  let cancelled = false;
  const cleanups: (() => void)[] = [];
  for (const image of root.querySelectorAll<HTMLImageElement>('img[data-local-src]')) {
    let url: string | undefined;
    let fallback: HTMLSpanElement | undefined;
    const originalHidden = image.hidden;
    const originalWidth = image.getAttribute('width'), originalHeight = image.getAttribute('height');
    const active = () => !cancelled && isCurrent() && root.isConnected && root.contains(image);
    const revoke = () => {
      if (!url) return;
      image.removeAttribute('src');
      URL.revokeObjectURL(url);
      url = undefined;
    };
    const fail = (error: unknown) => {
      if (!active() || fallback) return;
      revoke();
      image.removeAttribute('aria-busy');
      image.hidden = true;
      fallback = window.document.createElement('span');
      fallback.className = 'image-fallback';
      fallback.textContent = `${image.alt || 'Image'} — image unavailable: ${errorMessage(error)}`;
      fallback.setAttribute('role', 'img');
      fallback.setAttribute('aria-label', fallback.textContent);
      fallback.setAttribute('aria-live', 'polite');
      image.after(fallback);
    };
    const loaded = () => { if (active()) image.removeAttribute('aria-busy'); };
    const failed = () => fail(new Error('The local image could not be decoded.'));
    image.removeAttribute('src');
    image.setAttribute('aria-busy', 'true');
    image.addEventListener('load', loaded);
    image.addEventListener('error', failed);
    cleanups.push(() => {
      image.removeEventListener('load', loaded);
      image.removeEventListener('error', failed);
      revoke();
      image.removeAttribute('aria-busy');
      image.hidden = originalHidden;
      if (originalWidth === null) image.removeAttribute('width'); else image.setAttribute('width', originalWidth);
      if (originalHeight === null) image.removeAttribute('height'); else image.setAttribute('height', originalHeight);
      fallback?.remove();
    });
    void (async () => {
      try {
        const relative = image.dataset.localSrc ?? '';
        // Never assign a Markdown URL to src, even if the core contract is violated.
        if (!relative.trim() || /^(?:[a-z][a-z\d+.-]*:|[/?#\\])/i.test(relative.trim()) || relative.includes('\\')) {
          throw new Error('Only workspace-relative images can be loaded.');
        }
        const resolved = await api.resolve(document.workspace_id, document.path, relative);
        if (!active()) return;
        if (resolved.kind !== 'asset') throw new Error('The image did not resolve to a local asset.');
        const asset = await api.asset(document.workspace_id, resolved.path);
        if (!active()) return;
        const dimensions = imageDimensions(asset);
        if (dimensions && originalWidth === null && originalHeight === null) {
          image.width = dimensions.width; image.height = dimensions.height;
        }
        url = assetObjectUrl(asset);
        image.src = url;
      } catch (error) { fail(error); }
    })();
  }
  return () => { cancelled = true; cleanups.forEach(cleanup => cleanup()); };
}
