<script lang="ts">
  import FileTree from './FileTree.svelte';
  import type { TreeNode } from './helpers';
  import type { DocumentSummary } from './types';
  let { nodes, selected, open }: { nodes: TreeNode[]; selected: string | undefined; open: (document: DocumentSummary) => void } = $props();
</script>
<ul class="file-tree">
  {#each nodes as node (node.path)}
    <li>
      {#if node.document}
        <button class:chosen={selected === node.path} aria-current={selected === node.path ? 'page' : undefined} title={node.document.title} onclick={() => node.document && open(node.document)}><span class="file-icon" aria-hidden="true">▤</span><span>{node.name}</span></button>
      {:else}
        <details open><summary>{node.name}</summary><FileTree nodes={node.children} {selected} {open} /></details>
      {/if}
    </li>
  {/each}
</ul>
