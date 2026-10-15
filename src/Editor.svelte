<script lang="ts">
  import { onMount } from 'svelte';
  import { EditorState } from '@codemirror/state';
  import { EditorView, keymap, lineNumbers, drawSelection } from '@codemirror/view';
  import { defaultKeymap, history, historyKeymap } from '@codemirror/commands';
  import { markdown } from '@codemirror/lang-markdown';
  import { syntaxHighlighting, defaultHighlightStyle } from '@codemirror/language';
  import { search, searchKeymap, openSearchPanel } from '@codemirror/search';
  import { byteToPosition } from './helpers';
  let { content, onchange, onsave, offset = 0 }: { content: string; onchange: (value: string) => void; onsave: () => void; offset?: number } = $props();
  let host: HTMLDivElement;
  let view = $state.raw<EditorView>();
  $effect(() => {
    const value = content;
    if (view && view.state.doc.toString() !== value) {
      const anchor = Math.min(view.state.selection.main.anchor, value.length);
      view.dispatch({ changes: { from: 0, to: view.state.doc.length, insert: value }, selection: { anchor } });
    }
  });
  export function find() { if (view) openSearchPanel(view); }
  export function goTo(offset: number) {
    if (!view) return;
    const position = byteToPosition(view.state.doc.toString(), offset);
    view.dispatch({ selection: { anchor: position }, effects: EditorView.scrollIntoView(position, { y: 'center' }) });
    view.focus();
  }
  onMount(() => {
    view = new EditorView({ parent: host, state: EditorState.create({ doc: content, extensions: [
      lineNumbers(), drawSelection(), history(), markdown(), syntaxHighlighting(defaultHighlightStyle), search(), EditorView.lineWrapping,
      EditorView.contentAttributes.of({ 'aria-label': 'Markdown source', spellcheck: 'false' }),
      keymap.of([{ key: 'Mod-s', run: () => { onsave(); return true; } }, ...defaultKeymap, ...historyKeymap, ...searchKeymap]),
      EditorView.updateListener.of(update => { if (update.docChanged) onchange(update.state.doc.toString()); }),
      EditorView.theme({ '&': { height: '100%', background: 'transparent', color: 'var(--text)' }, '.cm-scroller': { overflow: 'auto', fontFamily: 'var(--mono)', fontSize: '14px', lineHeight: '1.8' }, '.cm-content': { padding: '28px 0' }, '.cm-gutters': { background: 'var(--panel)', color: 'var(--muted)', border: 'none' }, '.cm-cursor': { borderLeftColor: 'var(--text)' }, '.cm-activeLine, .cm-activeLineGutter': { background: 'var(--hover)' } })
    ] }) });
    goTo(offset);
    return () => view?.destroy();
  });
</script>
<div class="editor" bind:this={host}></div>
