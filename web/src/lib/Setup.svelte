<script lang="ts">
  import FolderPlus from '@lucide/svelte/icons/folder-plus'
  import FolderOpen from '@lucide/svelte/icons/folder-open'
  import { untrack } from 'svelte'
  import { runSetup, type SetupStatus } from './api'

  // First-run screen: start a new vault or use an existing folder of notes.
  let { status }: { status: SetupStatus } = $props()

  let mode = $state<'new' | 'existing'>('new')
  let newPath = $state(untrack(() => status.suggested) ?? '~/Quirenote') // initial suggestion; the field is free afterwards
  let existingPath = $state('')
  let error = $state('')
  let busy = $state(false)
  let done = $state<{ dir: string; obsidian: boolean } | null>(null)

  async function submit(e: SubmitEvent) {
    e.preventDefault()
    const path = (mode === 'new' ? newPath : existingPath).trim()
    if (!path) return (error = 'Type the folder path.')
    busy = true
    error = ''
    try {
      done = await runSetup(mode, path)
      // An Obsidian vault gets a moment to read the confirmation; otherwise go straight in.
      setTimeout(() => location.reload(), done.obsidian ? 1500 : 0)
    } catch (err) {
      error = (err as Error).message
      busy = false
    }
  }
</script>

<main class="setup">
  <form class="card" onsubmit={submit}>
    <h1>Welcome to Quirenote</h1>
    <p class="lead">Your notes are plain markdown files in a folder on this machine. Choose where they live.</p>

    <label class="option" class:selected={mode === 'new'}>
      <input type="radio" bind:group={mode} value="new" />
      <span class="title"><FolderPlus size={16} /> Start fresh</span>
      <span class="desc">Creates a new folder with a welcome note.</span>
      {#if mode === 'new'}
        <input class="path" bind:value={newPath} aria-label="New vault folder" spellcheck="false" />
      {/if}
    </label>

    <label class="option" class:selected={mode === 'existing'}>
      <input type="radio" bind:group={mode} value="existing" />
      <span class="title"><FolderOpen size={16} /> Use an existing folder</span>
      <span class="desc">
        An Obsidian vault works as is: nothing is copied, and your notes keep working in Obsidian, Neovim or any editor.
      </span>
      {#if mode === 'existing'}
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="path"
          bind:value={existingPath}
          placeholder="~/Documents/My vault"
          aria-label="Existing folder"
          spellcheck="false"
          autofocus
        />
      {/if}
    </label>

    {#if error}<p class="error">{error}</p>{/if}
    {#if done}
      <p class="ok">
        {done.obsidian ? 'Obsidian vault detected. ' : ''}Opening {done.dir}…
      </p>
    {/if}

    <div class="actions">
      <span class="hint">Paths are on the machine running the server; <code>~</code> is its home folder.</span>
      <button class="primary" type="submit" disabled={busy || !status.allowed}>
        {mode === 'new' ? 'Create vault' : 'Open folder'}
      </button>
    </div>
    {#if !status.allowed}
      <p class="error">Choosing the vault from the browser is disabled. Run <code>quirenote init &lt;folder&gt;</code>.</p>
    {/if}
  </form>
</main>

<style>
  .setup {
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 24px;
    box-sizing: border-box;
  }
  .card {
    width: min(560px, 100%);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 12px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.15);
    padding: 28px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  h1 {
    margin: 0;
    font-family: var(--editor-font);
    font-weight: 600;
    font-size: 30px;
    color: var(--heading);
  }
  .lead {
    margin: 0 0 6px;
    color: var(--muted);
    font-size: 14px;
  }
  .option {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 10px;
    row-gap: 4px;
    padding: 12px 14px;
    border: 1px solid var(--border);
    border-radius: 8px;
    cursor: pointer;
  }
  .option.selected {
    border-color: var(--accent);
    background: var(--hover);
  }
  .option > input[type='radio'] {
    grid-row: span 2;
    margin-top: 3px;
    accent-color: var(--accent);
  }
  .title {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-weight: 600;
    font-size: 14px;
  }
  .desc {
    grid-column: 2;
    font-size: 13px;
    color: var(--muted);
  }
  .path {
    grid-column: 2;
    margin-top: 6px;
    padding: 7px 10px;
    font-family: var(--mono);
    font-size: 13px;
    background: var(--bg);
  }
  .path:focus {
    outline: none;
    border-color: var(--accent);
  }
  .actions {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-top: 4px;
  }
  .hint {
    font-size: 12px;
    color: var(--muted);
  }
  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--panel);
    padding: 6px 14px;
    white-space: nowrap;
  }
  .primary:disabled {
    opacity: 0.6;
    cursor: default;
  }
  .error,
  .ok {
    margin: 0;
    font-size: 13px;
  }
  .error {
    color: var(--error);
  }
  .ok {
    color: var(--accent);
  }
  code {
    font-family: var(--mono);
    font-size: 12px;
  }
</style>
