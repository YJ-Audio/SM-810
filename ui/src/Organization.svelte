<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import QueryEditor from './QueryEditor.svelte';
  import Icon from './Icon.svelte';
  import type { Query, Rule, Collection, Root } from './types';
  let {
    rules,
    collections,
    roots,
    selection,
    selectedId,
    currentQuery,
    ondone,
  }: {
    rules: Rule[];
    collections: Collection[];
    roots: Root[];
    selection: number[];
    selectedId: number | null;
    currentQuery: () => Query;
    ondone: (deleted?: number) => Promise<void>;
  } = $props();
  let dialog: HTMLDialogElement;
  let mode = $state<'rules' | 'collection' | 'add'>('rules');
  let error = $state(''),
    busy = $state(false);
  let id = $state<number | null>(null),
    name = $state(''),
    kind = $state<'static' | 'smart'>('static');
  let query = $state<Query>({ type: 'all', conditions: [] });
  let destination = $state<number | null>(null);
  const blankRule = (): Rule => ({
    id: null,
    tag: '',
    target: 'filename',
    pattern: '',
    enabled: true,
  });
  let draft = $state<Rule>(blankRule());
  export function editCollection(collection?: Collection) {
    id = collection?.id ?? null;
    name = collection?.name ?? '';
    kind = collection?.kind ?? 'static';
    query = structuredClone(
      collection?.query ? $state.snapshot(collection.query) : currentQuery(),
    );
    error = '';
    mode = 'collection';
    dialog.showModal();
  }
  export function editRules() {
    draft = blankRule();
    error = '';
    mode = 'rules';
    dialog.showModal();
  }
  export function addSelection() {
    destination = collections.find((c) => c.kind === 'static')?.id ?? null;
    error = '';
    mode = 'add';
    dialog.showModal();
  }
  async function run(
    action: () => Promise<unknown>,
    close = false,
    deleted?: number,
  ) {
    busy = true;
    error = '';
    try {
      await action();
      await ondone(deleted);
      if (close) dialog.close();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }
  async function saveCollection() {
    await run(async () => {
      const saved = await invoke<number>('save_collection', {
        id,
        name,
        query: kind === 'smart' ? query : null,
      });
      if (!id && kind === 'static' && selection.length)
        await invoke('edit_collection_items', {
          id: saved,
          ids: selection,
          remove: false,
        });
    }, true);
  }
</script>

<dialog bind:this={dialog} class:wide-dialog={mode !== 'add'}>
  <div class="dialog-heading">
    <h2>
      {mode === 'rules'
        ? 'Tag rules'
        : mode === 'add'
          ? 'Add to collection'
          : id
            ? 'Edit collection'
            : 'New collection'}
    </h2>
    <button
      class="icon-button"
      aria-label="Close organizer"
      onclick={() => dialog.close()}><Icon name="close" /></button
    >
  </div>
  {#if error}<p class="organize-error" role="alert">{error}</p>{/if}
  {#if mode === 'rules'}
    <p>
      Apply tags from names and folders. Your manual tags stay in place when
      rules change.
    </p>
    <div class="rule-list">
      {#each rules as rule}<button
          class:active={draft.id === rule.id}
          onclick={() => (draft = { ...rule })}
          ><strong>{rule.tag}</strong><span class="mono">{rule.pattern}</span
          ><small>{rule.enabled ? 'On' : 'Off'}</small></button
        >{/each}
    </div>
    <button class="outline-button" onclick={() => (draft = blankRule())}
      >+ New rule</button
    >
    <form
      onsubmit={(e) => {
        e.preventDefault();
        void run(async () => {
          await invoke('save_rule', { rule: draft });
          if (!draft.id) draft = blankRule();
        });
      }}
    >
      <label for="rule-tag">Tag path</label><input
        id="rule-tag"
        bind:value={draft.tag}
        placeholder="Drums/kick"
        maxlength="512"
        required
      />
      <label for="rule-target">Match against</label><select
        id="rule-target"
        bind:value={draft.target}
        ><option value="filename">Filename</option><option value="rel_path"
          >Path within source</option
        ></select
      >
      <label for="rule-pattern">Regular expression</label><input
        id="rule-pattern"
        class="mono"
        bind:value={draft.pattern}
        placeholder="(?i)(kick|kck)"
        maxlength="4096"
        required
      />
      <small class="muted"
        >Use (?i) for case-insensitive matching. For folders: (?i)drums/</small
      >
      <label class="checkbox-label"
        ><input type="checkbox" bind:checked={draft.enabled} />Enabled</label
      >
      <div class="dialog-actions">
        <button class="primary-button" type="submit" disabled={busy}
          >Save & apply</button
        >
        {#if draft.id}<button
            type="button"
            class="outline-button"
            disabled={busy}
            onclick={() =>
              run(async () => {
                await invoke('delete_rule', { id: draft.id });
                draft = blankRule();
              })}>Delete rule</button
          >{/if}
        <button
          type="button"
          class="outline-button"
          disabled={busy}
          onclick={() => run(() => invoke('apply_rules'))}
          >Reapply all rules</button
        >
      </div>
    </form>
  {:else if mode === 'collection'}
    <form
      onsubmit={(e) => {
        e.preventDefault();
        void saveCollection();
      }}
    >
      <label for="collection-name">Collection name</label><input
        id="collection-name"
        bind:value={name}
        maxlength="100"
        required
      />
      <label for="collection-kind">Contents</label><select
        id="collection-kind"
        bind:value={kind}
        disabled={id !== null}
        ><option value="static">Saved selection · manual order</option><option
          value="smart">Smart · follows conditions</option
        ></select
      >
      {#if kind === 'smart'}<p>Membership updates as your library changes.</p>
        <QueryEditor bind:value={query} {roots} {selectedId} />
      {:else}<p>
          {id
            ? 'Add sounds using + Collection in the selection toolbar. Use ↑ / ↓ there to arrange them.'
            : `${selection.length} selected sounds will be added. You can add more later.`}
        </p>{/if}
      <div class="dialog-actions">
        <button class="primary-button" type="submit" disabled={busy}
          >Save collection</button
        >
        {#if id}<button
            class="outline-button"
            type="button"
            disabled={busy}
            onclick={() =>
              run(() => invoke('delete_collection', { id }), true, id!)}
            >Delete collection</button
          >{/if}
      </div>
    </form>
  {:else}
    <form
      onsubmit={(e) => {
        e.preventDefault();
        void run(
          () =>
            invoke('edit_collection_items', {
              id: destination,
              ids: selection,
              remove: false,
            }),
          true,
        );
      }}
    >
      <p>Add {selection.length} selected sounds to a saved collection.</p>
      <label for="add-collection">Collection</label><select
        id="add-collection"
        bind:value={destination}
        >{#each collections.filter((c) => c.kind === 'static') as collection}<option
            value={collection.id}>{collection.name}</option
          >{/each}</select
      >
      <div class="dialog-actions">
        <button
          class="primary-button"
          type="submit"
          disabled={!destination || busy}>Add selection</button
        ><button
          class="outline-button"
          type="button"
          onclick={() => editCollection()}>New collection</button
        >
      </div>
    </form>
  {/if}
</dialog>
