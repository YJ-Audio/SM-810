<script lang="ts">
  import type { Query, Root } from './types';
  let {
    value = $bindable<Query>({ type: 'all', conditions: [] }),
    roots,
    selectedId = null,
  }: {
    value?: Query;
    roots: Root[];
    selectedId?: number | null;
  } = $props();
  function fresh(type: Query['type']): Query {
    switch (type) {
      case 'all':
      case 'any':
        return { type, conditions: [] };
      case 'not':
        return { type, condition: { type: 'tag', name: '' } };
      case 'tag':
        return { type, name: '' };
      case 'text':
        return { type, text: '' };
      case 'root':
        return { type, id: roots[0]?.id ?? 0 };
      case 'collection':
        return { type, id: 0 };
      case 'field':
        return { type, field: 'duration_ms', op: 'lt', value: 200 };
      case 'similar_to':
        return { type, id: selectedId ?? 0, count: 50 };
      case 'semantic':
        return { type, text: '', count: 100 };
    }
  }
</script>

{#snippet editor(q: Query, change: (q: Query) => void, depth: number)}
  <div class="query-node">
    <select
      aria-label="Condition type"
      value={q.type}
      onchange={(e) => change(fresh(e.currentTarget.value as Query['type']))}
    >
      <option value="all">Match all conditions</option><option value="any"
        >Match any condition</option
      ><option value="not">Exclude condition</option>
      <option value="tag">Tag and descendants</option><option value="text"
        >Name / path / tags</option
      ><option value="root">Source</option>
      <option value="field">Audio property</option><option value="similar_to"
        >Similar to sample</option
      ><option value="semantic">Sound description</option>
      {#if q.type === 'collection'}<option value="collection"
          >Saved collection</option
        >{/if}
    </select>
    {#if q.type === 'all' || q.type === 'any'}
      {#each q.conditions as child, i}
        <div class="query-child">
          {@render editor(
            child,
            (next) =>
              change({
                ...q,
                conditions: q.conditions.map((item, at) =>
                  at === i ? next : item,
                ),
              }),
            depth + 1,
          )}
          <button
            type="button"
            class="remove-condition"
            aria-label="Remove condition"
            onclick={() =>
              change({
                ...q,
                conditions: q.conditions.filter((_, at) => at !== i),
              })}>×</button
          >
        </div>
      {/each}
      {#if depth < 8}<button
          type="button"
          class="outline-button"
          onclick={() =>
            change({
              ...q,
              conditions: [...q.conditions, { type: 'tag', name: '' }],
            })}>+ Condition</button
        >{/if}
      {#if !q.conditions.length}<small class="muted"
          >{q.type === 'all'
            ? 'All sounds until you add a condition.'
            : 'Add a condition to match sounds.'}</small
        >{/if}
    {:else if q.type === 'not'}
      {#if depth < 32}{@render editor(
          q.condition,
          (condition) => change({ type: 'not', condition }),
          depth + 1,
        )}{/if}
    {:else if q.type === 'tag'}
      <input
        aria-label="Condition tag"
        placeholder="Drums/kick"
        value={q.name}
        required
        onchange={(e) => change({ ...q, name: e.currentTarget.value })}
      />
    {:else if q.type === 'text' || q.type === 'semantic'}
      <input
        aria-label="Condition text"
        placeholder={q.type === 'semantic' ? 'A dusty, soft kick drum' : 'kick'}
        value={q.text}
        required
        onchange={(e) => change({ ...q, text: e.currentTarget.value })}
      />
      {#if q.type === 'semantic'}<label
          >Top matches<input
            aria-label="Sound match count"
            type="number"
            min="1"
            max="100000"
            value={q.count}
            onchange={(e) =>
              change({ ...q, count: Number(e.currentTarget.value) })}
          /></label
        >{/if}
    {:else if q.type === 'root'}
      <select
        aria-label="Condition source"
        value={q.id}
        onchange={(e) => change({ ...q, id: Number(e.currentTarget.value) })}
        >{#each roots as root}<option value={root.id}>{root.label}</option
          >{/each}</select
      >
    {:else if q.type === 'field'}
      <div class="query-fields">
        <select
          aria-label="Audio property"
          value={q.field}
          onchange={(e) =>
            change({ ...q, field: e.currentTarget.value as typeof q.field })}
        >
          <option value="duration_ms">Length (ms)</option><option value="bpm"
            >BPM</option
          ><option value="lufs">Loudness (LUFS)</option><option value="key_root"
            >Root (C=0…B=11)</option
          ><option value="is_loop">Loop (yes=1 / no=0)</option>
        </select>
        <select
          aria-label="Comparison"
          value={q.op}
          onchange={(e) =>
            change({ ...q, op: e.currentTarget.value as typeof q.op })}
          ><option value="eq">=</option><option value="ne">≠</option><option
            value="lt">&lt;</option
          ><option value="le">≤</option><option value="gt">&gt;</option><option
            value="ge">≥</option
          ></select
        >
        <input
          aria-label="Property value"
          type="number"
          step="any"
          value={q.value}
          required
          onchange={(e) =>
            change({ ...q, value: Number(e.currentTarget.value) })}
        />
      </div>
    {:else if q.type === 'similar_to'}
      <div class="query-fields">
        <label
          >Sample ID<input
            aria-label="Similar sample ID"
            type="number"
            min="1"
            value={q.id}
            required
            onchange={(e) =>
              change({ ...q, id: Number(e.currentTarget.value) })}
          /></label
        >
        <label
          >Top matches<input
            aria-label="Similar match count"
            type="number"
            min="1"
            max="100000"
            value={q.count}
            required
            onchange={(e) =>
              change({ ...q, count: Number(e.currentTarget.value) })}
          /></label
        >
      </div>
      {#if selectedId}<button
          type="button"
          class="outline-button"
          onclick={() => change({ ...q, id: selectedId })}
          >Use selected sound</button
        >{/if}
    {:else if q.type === 'collection'}<small class="muted"
        >Membership follows collection #{q.id}.</small
      >{/if}
  </div>
{/snippet}
{@render editor(value, (q) => (value = q), 0)}
