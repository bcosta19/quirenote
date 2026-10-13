<script lang="ts">
  import { onMount, tick } from 'svelte'
  import RotateCcw from '@lucide/svelte/icons/rotate-ccw'
  import {
    FONT_DEFAULTS,
    FONT_HINTS,
    FONT_LABELS,
    FONT_ROLES,
    cleanFontName,
    fontAvailable,
    keepFontChars,
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

  let opening = false

  // The field of each role, to focus the first one when the dialog opens.
  const fields = $state<Partial<Record<FontRole, HTMLInputElement>>>({})

  onMount(async () => {
    // Typing then replaces the current name instead of appending to it. The suggestions stay
    // closed on this first focus: they would cover the other fields as soon as the dialog opens.
    opening = true
    fields[FONT_ROLES[0]]?.select()
    opening = false
    installed = await listSystemFonts()
    listing = false
  })

  // A name the browser cannot draw would fall back to the default without a word.
  const missing = (name: string) => !!name && !fontAvailable(name)

  /** Keeps only what can be part of a font name, and shows it in the field. */
  function set(e: Event, role: FontRole) {
    const input = e.currentTarget as HTMLInputElement
    fonts[role] = cleanFontName(input.value)
    openList(role, true)
    // A character dropped here would stay on screen until the next keystroke: Svelte only
    // rewrites the field when the name changes, and dropping one does not change it.
    if (keepFontChars(input.value) !== input.value) {
      const caret = keepFontChars(input.value.slice(0, input.selectionStart ?? 0)).length
      input.value = cleanFontName(input.value)
      input.setSelectionRange(caret, caret)
    }
  }

  // The suggestions of one field at a time: the role whose list is open, whether it filters by
  // the typed text (just focused: all the fonts) and the highlighted font.
  let openRole = $state<FontRole | null>(null)
  let filtering = $state(false)
  let activeName = $state<string | null>(null)

  const suggestions = $derived.by(() => {
    if (!openRole) return []
    const query = filtering ? fonts[openRole].toLowerCase() : ''
    return installed.filter((name) => name.toLowerCase().includes(query))
  })
  // Nothing to show (no fonts to list, or none matching) means no list at all.
  const listOpen = (role: FontRole) => role === openRole && suggestions.length > 0
  const activeIndex = $derived(activeName ? suggestions.indexOf(activeName) : -1)

  const optionId = (role: FontRole, i: number) => `font-${role}-option-${i}`

  function openList(role: FontRole, filter: boolean) {
    openRole = role
    filtering = filter
    // A name that is already one of the fonts starts as the highlighted one.
    activeName = installed.includes(fonts[role]) ? fonts[role] : null
    reveal()
  }

  /** Keeps the highlighted font in view when the list scrolls (the keyboard moves it past the edge). */
  async function reveal() {
    await tick()
    if (openRole && activeIndex >= 0) {
      document.getElementById(optionId(openRole, activeIndex))?.scrollIntoView({ block: 'nearest' })
    }
  }

  function choose(role: FontRole, name: string) {
    fonts[role] = name
    openRole = null
  }

  function move(delta: number) {
    const last = suggestions.length - 1
    const next = activeIndex < 0 ? (delta > 0 ? 0 : last) : Math.min(last, Math.max(0, activeIndex + delta))
    activeName = suggestions[next]
    reveal()
  }

  /** The list keys of a field, before the dialog's own handler sees the event. */
  function fieldKeydown(e: KeyboardEvent, role: FontRole) {
    if (e.altKey || e.ctrlKey || e.metaKey) return
    const open = listOpen(role)
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
      e.preventDefault()
      // A closed list (after Escape or a choice) comes back with the arrows.
      if (!open) openList(role, false)
      else move(e.key === 'ArrowDown' ? 1 : -1)
    } else if (e.key === 'Enter' && open && activeName) {
      e.preventDefault()
      choose(role, activeName)
    } else if (e.key === 'Escape' && open) {
      // Closes only the list: the dialog stays, and the next Escape closes it.
      e.preventDefault()
      e.stopPropagation()
      openRole = null
    }
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
  <div class="fonts-dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-modal="true" aria-label="Fonts" onkeydown={onkeydown} tabindex="-1">
    <h2>Fonts</h2>
    <p class="hint">
      {#if listing}
        Asking the browser for the fonts installed on this device…
      {:else if installed.length}
        {installed.length} font{installed.length === 1 ? '' : 's'} installed on this device. Any name also works, even one the
        browser does not list.
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
        <div class="field">
          <!-- The field itself uses the font it is choosing, so the choice is visible. -->
          <input
            id="font-{role}"
            bind:this={fields[role]}
            value={fonts[role]}
            oninput={(e) => set(e, role)}
            onfocus={() => !opening && openList(role, false)}
            onclick={() => !listOpen(role) && openList(role, false)}
            onblur={() => openRole === role && (openRole = null)}
            onkeydown={(e) => fieldKeydown(e, role)}
            placeholder="Default ({FONT_DEFAULTS[role]})"
            aria-describedby={missing(fonts[role]) ? `font-${role}-missing` : undefined}
            role="combobox"
            aria-expanded={listOpen(role)}
            aria-controls={listOpen(role) ? `font-${role}-list` : undefined}
            aria-autocomplete="list"
            aria-activedescendant={listOpen(role) && activeIndex >= 0 ? optionId(role, activeIndex) : undefined}
            spellcheck="false"
            autocomplete="off"
          />
          {#if listOpen(role)}
            <!-- Drawn here instead of a <datalist>: that popup ignores the theme, covers the
                 dialog and shows every name in the same font. Each option shows its own font.
                 mousedown must not take the focus from the field (nor on the scroll bar). -->
            <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
            <ul class="suggestions" id="font-{role}-list" role="listbox" aria-label="{FONT_LABELS[role]} fonts" onmousedown={(e) => e.preventDefault()}>
              {#each suggestions as name, i (name)}
                <!-- svelte-ignore a11y_click_events_have_key_events -->
                <li
                  id={optionId(role, i)}
                  role="option"
                  aria-selected={i === activeIndex}
                  class:active={i === activeIndex}
                  class:current={name === fonts[role]}
                  style:font-family="'{name}', var(--font-{role}-tail)"
                  onmousemove={() => (activeName = name)}
                  onclick={() => choose(role, name)}
                >
                  {name}
                </li>
              {/each}
            </ul>
          {/if}
        </div>
        <button
          class="reset"
          onclick={() => (fonts[role] = '')}
          disabled={!fonts[role]}
          title="Default font for the {FONT_LABELS[role].toLowerCase()}"
          aria-label="Default font for the {FONT_LABELS[role].toLowerCase()}"><RotateCcw size={14} /></button
        >
        {#if missing(fonts[role])}
          <p class="missing" id="font-{role}-missing">Not found on this device: the default font is shown instead.</p>
        {/if}
      </div>
    {/each}

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
  /* Not `.dialog`: pdf_viewer.css (pdf.js) styles that class globally, color and font included. */
  .fonts-dialog {
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
    grid-template-columns: 1fr auto;
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
  /* The anchor of the suggestions: they open right under the field, inside the dialog. */
  .field {
    position: relative;
    min-width: 0;
  }
  input {
    width: 100%;
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
  .suggestions {
    position: absolute;
    top: calc(100% + 4px);
    left: 0;
    right: 0;
    z-index: 70;
    margin: 0;
    padding: 4px;
    list-style: none;
    max-height: min(240px, 40vh);
    overflow-y: auto;
    background: var(--bg);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 8px 24px rgba(0, 0, 0, 0.25);
  }
  .suggestions li {
    padding: 5px 8px;
    border-radius: 5px;
    font-size: 15px;
    line-height: 1.3;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    cursor: pointer;
  }
  .suggestions li.current {
    color: var(--accent);
  }
  .suggestions li.active {
    background: var(--accent-bg);
  }
  .missing {
    grid-column: 1 / -1;
    margin: 0;
    font-size: 12px;
    color: var(--warn);
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
