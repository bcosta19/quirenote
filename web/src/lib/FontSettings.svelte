<script lang="ts">
  import { onMount } from 'svelte'
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw'
  import {
    FONT_HINTS,
    FONT_LABELS,
    FONT_ROLES,
    cleanFontName,
    listSystemFonts,
    type FontPrefs,
    type FontRole,
  } from './fonts'

  // Picks the fonts installed on this device, per role (like the appearance settings of any
  // editor). Changes apply while the dialog is open; they are saved per device.

  let {
    fonts = $bindable(),
    onclose,
  }: {
    fonts: FontPrefs
    onclose: () => void
  } = $props()

  // Fonts installed on this device, when the browser lets us ask for them.
  let installed = $state<string[]>([])
  let listing = $state(true)

  onMount(async () => {
    document.getElementById('font-ui')?.focus()
    installed = await listSystemFonts()
    listing = false
  })

  /** Keeps only what can be part of a font name, and shows it in the field. */
  function set(e: Event, role: FontRole) {
    const input = e.currentTarget as HTMLInputElement
    fonts[role] = cleanFontName(input.value)
  }

  function onkeydown(e: KeyboardEvent) {
    // Keeps the app shortcuts (Ctrl+O, Ctrl+E…) from acting while typing here.
    e.stopPropagation()
    if (e.key === 'Escape') {
      e.preventDefault()
      onclose()
    }
  }
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={onclose}>
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label="Fonts" onkeydown={onkeydown} tabindex="-1">
    <h2>Fonts</h2>
    <p class="hint">
      {#if listing}
        Asking the browser for the fonts installed on this device…
      {:else if installed.length}
        {installed.length} fonts installed on this device. Any name also works, even one the browser does not list.
      {:else}
        This browser cannot list the fonts installed on this device (or the permission was refused): type the name, as in “Inter”.
      {/if}
    </p>

    {#each FONT_ROLES as role (role)}
      <div class="row">
        <label for="font-{role}">
          <span class="name">{FONT_LABELS[role]}</span>
          <span class="hint">{FONT_HINTS[role]}</span>
        </label>
        <!-- The field itself uses the font it is choosing, so the choice is visible. -->
        <input
          id="font-{role}"
          value={fonts[role]}
          oninput={(e) => set(e, role)}
          placeholder="Default"
          list="system-fonts"
          spellcheck="false"
          autocomplete="off"
        />
        <button
          class="reset"
          onclick={() => (fonts[role] = '')}
          disabled={!fonts[role]}
          title="Default font for the {FONT_LABELS[role].toLowerCase()}"
          aria-label="Default font for the {FONT_LABELS[role].toLowerCase()}"><RotateCcw size={14} /></button
        >
      </div>
    {/each}

    <datalist id="system-fonts">
      {#each installed as name (name)}
        <option value={name}>{name}</option>
      {/each}
    </datalist>

    <div class="actions">
      <button
        onclick={() => {
          for (const role of FONT_ROLES) fonts[role] = ''
        }}
        disabled={!FONT_ROLES.some((role) => fonts[role])}>Reset all</button
      >
      <button class="primary" onclick={onclose}>Done</button>
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    background: rgba(0, 0, 0, 0.25);
    display: flex;
    justify-content: center;
    align-items: flex-start;
    padding-top: 18vh;
  }
  .dialog {
    width: min(480px, 92vw);
    background: var(--panel);
    border: 1px solid var(--border);
    border-radius: 10px;
    box-shadow: 0 12px 40px rgba(0, 0, 0, 0.25);
    padding: 16px;
  }
  h2 {
    margin: 0 0 4px;
    font-size: 15px;
  }
  .hint {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
    line-height: 1.5;
  }
  .row {
    display: grid;
    grid-template-columns: 1fr auto auto;
    gap: 4px 8px;
    align-items: center;
    margin-top: 12px;
  }
  label {
    grid-column: 1 / -1;
    display: flex;
    flex-direction: column;
    gap: 1px;
  }
  .name {
    font-size: 13px;
  }
  input {
    min-width: 0;
    font-family: var(--ui-font);
    padding: 5px 8px;
    background: var(--bg);
  }
  /* The field shows the font it chooses. */
  #font-editor {
    font-family: var(--editor-font);
    font-size: 15px;
  }
  #font-mono {
    font-family: var(--mono);
  }
  input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .reset {
    display: inline-flex;
    align-items: center;
    padding: 5px 7px;
  }
  .reset:disabled {
    opacity: 0.4;
    cursor: default;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 16px;
  }
  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--panel);
  }
</style>