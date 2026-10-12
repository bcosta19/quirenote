<script lang="ts">
  import BookOpen from '@lucide/svelte/icons/book-open'
  import BookText from '@lucide/svelte/icons/book-text'
  import FileText from '@lucide/svelte/icons/file-text'

  // Icon by file type: PDF, EPUB or note.
  let { name, size = 14 }: { name: string; size?: number } = $props()
</script>

{#if /\.pdf$/i.test(name)}
  <BookOpen {size} />
{:else if /\.epub$/i.test(name)}
  <BookText {size} />
{:else}
  <FileText {size} />
{/if}
