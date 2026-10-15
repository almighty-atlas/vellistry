import { afterEach, vi } from 'vitest';
import { cleanup } from '@testing-library/svelte';

// jsdom does not implement native dialog display or browser layout APIs.
if (!HTMLDialogElement.prototype.showModal) HTMLDialogElement.prototype.showModal = function () { this.open = true; };
if (!HTMLDialogElement.prototype.close) HTMLDialogElement.prototype.close = function () { this.open = false; this.dispatchEvent(new Event('close')); };
if (!URL.createObjectURL) URL.createObjectURL = vi.fn(() => 'blob:local-image');
if (!URL.revokeObjectURL) URL.revokeObjectURL = vi.fn();
if (!HTMLElement.prototype.scrollIntoView) HTMLElement.prototype.scrollIntoView = vi.fn();
if (!Range.prototype.getClientRects) Range.prototype.getClientRects = () => [] as unknown as DOMRectList;
if (!Range.prototype.getBoundingClientRect) Range.prototype.getBoundingClientRect = () => new DOMRect();
Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText: vi.fn().mockResolvedValue(undefined) } });
afterEach(() => { cleanup(); localStorage.clear(); vi.restoreAllMocks(); });
