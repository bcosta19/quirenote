<script lang="ts">
  import { onMount } from 'svelte'

  // Substitui o prompt() nativo do navegador, que não segue o tema do app.
  let {
    message,
    value = '',
    hint = '',
    input: withInput = true,
    okLabel = 'OK',
    danger = false,
    onsubmit,
    oncancel,
  }: {
    message: string
    value?: string
    hint?: string
    /** false: só confirmação (sem campo de texto). */
    input?: boolean
    okLabel?: string
    danger?: boolean
    onsubmit: (value: string) => void
    oncancel: () => void
  } = $props()

  let input = $state<HTMLInputElement>()
  let ok: HTMLButtonElement
  // svelte-ignore state_referenced_locally
  let text = $state(value) // valor inicial sugerido; depois o campo é livre

  function onkeydown(e: KeyboardEvent) {
    // Não deixa atalhos do app (Ctrl+O, Ctrl+E…) agirem enquanto digita aqui.
    e.stopPropagation()
    if (e.key === 'Enter') {
      e.preventDefault()
      onsubmit(text)
    } else if (e.key === 'Escape') {
      e.preventDefault()
      oncancel()
    }
  }

  onMount(() => {
    if (!input) {
      ok.focus()
      return
    }
    input.focus()
    // Seleciona só o nome (sem pasta nem .md), como ao renomear no explorador de arquivos.
    const start = text.lastIndexOf('/') + 1
    const end = text.toLowerCase().endsWith('.md') ? text.length - 3 : text.length
    input.setSelectionRange(start, end)
  })
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class="backdrop" onclick={oncancel}>
  <div class="dialog" onclick={(e) => e.stopPropagation()} role="dialog" aria-label={message} tabindex="-1">
    {#if withInput}
      <label>
        <span class="message">{message}</span>
        <input bind:this={input} bind:value={text} {onkeydown} />
      </label>
    {:else}
      <p class="message">{message}</p>
    {/if}
    {#if hint}<p class="hint">{hint}</p>{/if}
    <div class="actions">
      <button onclick={oncancel}>Cancelar</button>
      <button
        class="primary"
        class:danger
        bind:this={ok}
        onclick={() => onsubmit(text)}
        onkeydown={(e) => {
          e.stopPropagation()
          if (e.key === 'Escape') oncancel()
        }}>{okLabel}</button
      >
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
  label {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }
  .message {
    font-size: 14px;
  }
  input {
    width: 100%;
    padding: 8px 10px;
    font-size: 14px;
    background: var(--bg);
  }
  input:focus {
    outline: none;
    border-color: var(--accent);
  }
  .hint {
    margin: 8px 0 0;
    font-size: 12px;
    color: var(--muted);
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    margin-top: 14px;
  }
  .primary {
    background: var(--accent);
    border-color: var(--accent);
    color: var(--panel);
  }
  .primary.danger {
    background: var(--error);
    border-color: var(--error);
  }
  p.message {
    margin: 0;
  }
</style>
