<script lang="ts">
  import { onMount } from 'svelte';
  import { invoke, isTauri } from '@tauri-apps/api/core';
  import { listen } from '@tauri-apps/api/event';
  import { open } from '@tauri-apps/plugin-dialog';
  import { startDrag } from '@crabnebula/tauri-plugin-drag';
  import Icon from './Icon.svelte';
  import Waveform from './Waveform.svelte';
  import MapCanvas from './MapCanvas.svelte';
  import Organization from './Organization.svelte';
  let organizer = $state<ReturnType<typeof Organization>>();
  let mapCanvas = $state<ReturnType<typeof MapCanvas>>();
  import { keys, kind, parseSearch } from './types';
  import type {
    Sample,
    Page,
    Root,
    Tag,
    Job,
    Bootstrap,
    Wave,
    Playback,
    MapSummary,
    Query,
    Rule,
    Collection,
  } from './types';

  let collections = $state<Collection[]>([]),
    rules = $state<Rule[]>([]),
    activeCollection = $state<number | null>(null);
  let currentCollection = $derived(
    collections.find((c) => c.id === activeCollection),
  );
  let maps = $state<MapSummary[]>([]),
    activeMap = $state<number | null>(null),
    mapInfo = $state<MapSummary | null>(null),
    mapBuffer = $state.raw<ArrayBuffer | null>(null),
    layoutActive = $state(0);
  let coloring = $state(0),
    loudnessSize = $state(true),
    traceLabel = $state(''),
    traceId = 0,
    traceRevision = 0,
    mapName = $state('');
  let modelReady = $state(false),
    embedding = $state(false),
    embedded = $state(0),
    downloading = $state(false),
    downloadPercent = $state(0);
  let neighbors = $state<Sample[]>([]),
    findingSimilar = $state(false),
    similarError = $state('');
  let sortBySimilarity = $state(true),
    similarityAnchor = $state<number | null>(null);
  let neighborRequest = 0;
  let roots = $state<Root[]>([]),
    tags = $state<Tag[]>([]),
    jobs = $state<Job[]>([]);
  let rows = $state<Sample[]>([]),
    total = $state(0),
    offset = $state(0),
    loading = $state(true),
    analyzing = $state(false);
  let query = $state(''),
    activeRoot = $state<number | null>(null),
    tagFilter = $state<string | null>(null);
  let mode = $state<'list' | 'map'>('list'),
    selected = $state<Sample | null>(null),
    selection = $state<number[]>([]),
    anchor = $state(0);
  let wave = $state<Wave | null>(null),
    sliceStart = $state(0),
    sliceEnd = $state(0),
    slicePath = $state<string | null>(null),
    preparing = $state(false);
  let playback = $state<Playback>({
    sample_id: 0,
    seconds: 0,
    playing: false,
    error: null,
  });
  let linkTempo = $state(124);
  let matchLufs = $state(true),
    semitones = $state(0),
    targetLufs = $state(-16),
    error = $state(''),
    busy = $state('');
  let columnsOpen = $state(false),
    showBpm = $state(false),
    showTags = $state(true),
    showRoot = $state(true);
  let searchInput = $state<HTMLInputElement>(undefined!),
    scroller = $state<HTMLDivElement>(undefined!),
    dialog = $state<HTMLDialogElement>(undefined!),
    waveBox = $state<HTMLDivElement>(undefined!);
  let dialogMode = $state<'tag' | 'settings' | 'source' | 'map'>('tag'),
    tagName = $state(''),
    sourcePath = $state(''),
    sourceLabel = $state(''),
    storage = $state('local');
  let metaBpm = $state(''),
    metaKey = $state(''),
    metaMode = $state('');
  let request = 0,
    detailRequest = 0,
    sliceRequest = 0,
    searchTimer: ReturnType<typeof setTimeout>,
    scrollTimer: ReturnType<typeof setTimeout>;
  let prefetchAt = 0;
  const rowHeight = 44,
    pageSize = 180;
  let parsed = $derived(parseSearch(query));
  let activeTag = $derived(tagFilter ?? parsed.tag);
  let selectedKind = $derived(selected ? kind(selected) : null);
  let embeddingFailed = $derived(
    jobs
      .filter((j) => j.kind === 'embed' && j.state === 'failed')
      .reduce((sum, j) => sum + j.count, 0),
  );
  let analysisTotal = $derived(
    jobs.filter((j) => j.kind === 'analyze').reduce((n, j) => n + j.count, 0),
  );
  let analysisDone = $derived(
    jobs
      .filter((j) => j.kind === 'analyze' && j.state === 'done')
      .reduce((n, j) => n + j.count, 0),
  );
  let analysisFailed = $derived(
    jobs
      .filter((j) => j.kind === 'analyze' && j.state === 'failed')
      .reduce((n, j) => n + j.count, 0),
  );
  let viewportWidth = $state(1480);
  let columnTemplate = $derived(
    viewportWidth <= 1200
      ? `65px minmax(100px,1.7fr) ${showTags ? 'minmax(60px,1fr) ' : ''}${showRoot ? '40px ' : ''}55px 45px${showBpm ? ' 45px' : ''}`
      : `112px minmax(170px,1.7fr) ${showTags ? 'minmax(100px,1fr) ' : ''}${showRoot ? '64px ' : ''}80px 75px${showBpm ? ' 65px' : ''}`,
  );
  let progress = $derived(
    selected && wave && playback.sample_id === selected.id
      ? Math.min(
          1,
          (playback.seconds * Math.pow(2, semitones / 12)) /
            (wave.frames / wave.sample_rate),
        )
      : 0,
  );
  const count = (n: number) => n.toLocaleString();
  const decimal = (n: number | null | undefined, d = 1) =>
    n == null ? '—' : n.toFixed(d);
  const settings = () => ({
    match_lufs: matchLufs,
    semitones,
    target_lufs: targetLufs,
  });
  const fail = (e: unknown) => {
    error = String(e);
  };
  const browseQuery = (start = 0, limit = pageSize) => ({
    text: parsed.text,
    tag: activeTag,
    root_id: activeRoot,
    map_id: activeMap,
    collection_id: activeCollection,
    similar_to: sortBySimilarity && !parsed.semantic ? similarityAnchor : null,
    offset: start,
    limit,
  });

  async function refreshStatus() {
    const data = await invoke<Bootstrap>('bootstrap');
    maps = data.maps;
    collections = data.collections;
    rules = data.rules;
    layoutActive = data.layout_active;
    roots = data.roots;
    tags = data.tags;
    jobs = data.jobs;
    analyzing = data.analyzing;
    modelReady = data.model_ready;
    embedding = data.embedding;
    embedded = data.embedded;
    downloading = data.downloading;
    downloadPercent = data.download_total
      ? (data.download_bytes / data.download_total) * 100
      : 0;
    if (data.audio_error) error = data.audio_error;
  }
  async function findSimilar(sample: Sample) {
    const revision = ++neighborRequest;
    neighbors = [];
    similarError = '';
    if (!modelReady && !embedded) return;
    findingSimilar = true;
    try {
      const results = await invoke<Sample[]>('similar', { id: sample.id });
      if (revision === neighborRequest) neighbors = results;
    } catch (e) {
      if (revision === neighborRequest) similarError = String(e);
    } finally {
      if (revision === neighborRequest) findingSimilar = false;
    }
  }
  async function indexSounds() {
    try {
      if (!modelReady) {
        downloading = true;
        await invoke('download_model');
        await refreshStatus();
      }
      await invoke('start_embedding');
      embedding = true;
    } catch (e) {
      fail(e);
    } finally {
      await refreshStatus().catch(fail);
    }
  }
  async function repairModel() {
    downloading = true;
    try {
      await invoke('download_model');
    } catch (e) {
      fail(e);
    } finally {
      await refreshStatus().catch(fail);
    }
  }
  async function pauseIndexing() {
    await invoke('pause_embedding').catch(fail);
  }
  async function loadMap() {
    if (!activeMap) activeMap = maps[0]?.id ?? null;
    if (!activeMap) return;
    const revision = ++request;
    loading = true;
    try {
      const result = await invoke<ArrayBuffer>('map_points', {
        id: activeMap,
        query: browseQuery(),
      });
      if (revision !== request) return;
      mapBuffer = result;
      total = new DataView(result).getUint32(24, true);
      const info = await invoke<MapSummary>('map_summary', { id: activeMap });
      if (revision === request) mapInfo = info;
    } catch (e) {
      if (revision === request) fail(e);
    } finally {
      if (revision === request) loading = false;
    }
  }
  function showMap() {
    mode = 'map';
    void loadMap();
  }
  function chooseMap(id: number) {
    activeMap = id;
    mapBuffer = null;
    mapInfo = null;
    void loadMap();
  }
  async function selectFromMap(id: number) {
    const revision = ++traceRevision;
    try {
      const page = await invoke<Page>('sample', { id });
      if (revision === traceRevision && page.items[0])
        await select(page.items[0], 0, false, false, false);
    } catch (e) {
      fail(e);
    }
  }
  function trace(id: number | null) {
    if (id === null) {
      stop();
      traceLabel = '';
      return;
    }
    traceId = id;
    const issuedMs = Date.now();
    void invoke('audition', {
      id,
      settings: settings(),
      issuedMs,
      probe: issuedMs,
    }).catch(fail);
    void invoke<Page>('sample', { id })
      .then((page) => {
        if (traceId === id) traceLabel = page.items[0]?.name ?? '';
      })
      .catch(() => {});
  }
  function prefetchMap(ids: number[]) {
    void invoke('prefetch', { ids, settings: settings() }).catch(() => {});
  }
  function mapSelection(ids: number[]) {
    selection = ids;
  }
  function currentMapQuery(): Query {
    const conditions: Query[] = [];
    if (activeCollection)
      conditions.push({ type: 'collection', id: activeCollection });
    if (activeMap) {
      const query = maps.find((m) => m.id === activeMap)?.query;
      if (query) conditions.push(query);
    }
    if (activeRoot) conditions.push({ type: 'root', id: activeRoot });
    if (activeTag) conditions.push({ type: 'tag', name: activeTag });
    if (parsed.text)
      conditions.push(
        parsed.semantic
          ? { type: 'semantic', text: parsed.text.slice(1).trim(), count: 200 }
          : { type: 'text', text: parsed.text },
      );
    return { type: 'all', conditions };
  }
  async function saveMap() {
    dialog.close();
    try {
      await invoke('create_map', { name: mapName, query: currentMapQuery() });
      await refreshStatus();
      activeMap = maps.at(-1)?.id ?? null;
      mode = 'map';
      await loadMap();
    } catch (e) {
      fail(e);
    }
  }
  async function recompute() {
    if (!activeMap) return;
    try {
      await invoke('start_layout', { id: activeMap });
      layoutActive = activeMap;
    } catch (e) {
      fail(e);
    }
  }
  async function load(start = 0) {
    if (mode === 'map') return loadMap();
    const revision = ++request;
    loading = true;
    try {
      const page = await invoke<Page>('browse', { query: browseQuery(start) });
      if (revision !== request) return;
      rows = page.items;
      total = page.total;
      offset = start;
      if (selected) {
        const updated = rows.find((r) => r.id === selected?.id);
        if (updated) selected = updated;
      }
    } catch (e) {
      if (revision === request) {
        fail(e);
        rows = [];
        total = 0;
      }
    } finally {
      if (revision === request) loading = false;
    }
  }
  function searchChanged() {
    clearTimeout(searchTimer);
    searchTimer = setTimeout(() => {
      if (scroller) scroller.scrollTop = 0;
      void load();
    }, 160);
  }
  function filterRoot(id: number | null) {
    activeRoot = id;
    if (scroller) scroller.scrollTop = 0;
    void load();
  }
  function filterTag(name: string | null) {
    tagFilter = name;
    if (scroller) scroller.scrollTop = 0;
    void load();
  }
  function scroll() {
    clearTimeout(scrollTimer);
    scrollTimer = setTimeout(() => {
      const top = Math.floor(scroller.scrollTop / rowHeight);
      if (top < offset || top > offset + pageSize - 40)
        void load(Math.max(0, top - 30));
    }, 50);
  }
  function prefetch(index: number) {
    if (Date.now() - prefetchAt < 150) return;
    prefetchAt = Date.now();
    const ids = [index, index + 1, index - 1, index + 2]
      .map((i) => rows[i - offset])
      .filter((sample) => sample?.available)
      .map((sample) => sample.id);
    void invoke('prefetch', { ids, settings: settings() }).catch(() => {});
  }
  function play(sample: Sample | null = selected) {
    if (!sample) return;
    if (!sample.available) {
      error = 'This sample is offline. Reconnect its source and rescan.';
      return;
    }
    void invoke('audition', { id: sample.id, settings: settings() }).catch(
      fail,
    );
  }
  function stop() {
    void invoke('audition', { id: null, settings: settings() }).catch(fail);
  }
  async function select(
    sample: Sample,
    index: number,
    range = false,
    reorder = true,
    autoplay = true,
  ) {
    const revision = ++detailRequest;
    if (range) {
      try {
        const page = await invoke<Page>('browse', {
          query: browseQuery(
            Math.min(anchor, index),
            Math.abs(index - anchor) + 1,
          ),
        });
        if (revision !== detailRequest) return;
        selection = page.items.map((r) => r.id);
      } catch (e) {
        fail(e);
      }
    } else {
      selection = [sample.id];
      anchor = index;
    }
    if (revision !== detailRequest) return;
    ++sliceRequest;
    selected = sample;
    wave = null;
    slicePath = null;
    preparing = false;
    metaBpm = sample.analysis?.bpm?.toString() ?? '';
    metaKey = sample.analysis?.key_root?.toString() ?? '';
    metaMode = sample.analysis?.key_mode ?? '';
    if (autoplay) play(sample);
    void findSimilar(sample);
    if (
      reorder &&
      !range &&
      sortBySimilarity &&
      currentCollection?.kind !== 'static' &&
      embedded > 0 &&
      !parsed.semantic
    ) {
      similarityAnchor = sample.id;
      if (scroller) scroller.scrollTop = 0;
      await load();
      anchor = rows.findIndex((r) => r.id === sample.id);
    }
    if (!sample.available) return;
    try {
      const result = await invoke<Wave>('waveform', { id: sample.id });
      if (revision !== detailRequest) return;
      wave = result;
      sliceStart = 0;
      sliceEnd = result.frames;
    } catch (e) {
      if (revision === detailRequest) fail(e);
    }
  }
  async function moveSelection(delta: number) {
    const current = rows.findIndex((r) => r.id === selected?.id);
    const index = Math.max(
      0,
      Math.min(total - 1, current >= 0 ? offset + current + delta : 0),
    );
    let sample = rows[index - offset];
    if (!sample) {
      const page = await invoke<Page>('browse', {
        query: browseQuery(Math.max(0, index - 30)),
      });
      rows = page.items;
      offset = Math.max(0, index - 30);
      sample = rows[index - offset];
    }
    if (sample) {
      void select(sample, index, false, false);
      prefetch(index);
      if (scroller) {
        if (index * rowHeight < scroller.scrollTop)
          scroller.scrollTop = index * rowHeight;
        else if (
          (index + 1) * rowHeight >
          scroller.scrollTop + scroller.clientHeight
        )
          scroller.scrollTop = (index + 1) * rowHeight - scroller.clientHeight;
      }
    }
  }
  function keyboard(event: KeyboardEvent) {
    if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
      event.preventDefault();
      searchInput?.focus();
      return;
    }
    if (
      document.querySelector('dialog[open]') ||
      (event.target instanceof HTMLElement &&
        ['INPUT', 'TEXTAREA', 'SELECT'].includes(event.target.tagName))
    )
      return;
    if (event.code === 'Space') {
      event.preventDefault();
      if (playback.playing) stop();
      else play();
    }
    if (event.key === 'Escape') {
      ++detailRequest;
      ++sliceRequest;
      selected = null;
      ++neighborRequest;
      neighbors = [];
      findingSimilar = false;
      similarityAnchor = null;
      void load();
      selection = [];
      wave = null;
      slicePath = null;
      stop();
    }
    if (
      mode === 'list' &&
      (event.key === 'ArrowDown' || event.key === 'ArrowUp')
    ) {
      event.preventDefault();
      void moveSelection(event.key === 'ArrowDown' ? 1 : -1).catch(fail);
    }
  }
  async function chooseSource() {
    try {
      const path = await open({
        directory: true,
        multiple: false,
        title: 'Add a sample source',
      });
      if (typeof path !== 'string') return;
      sourcePath = path;
      sourceLabel = path.split(/[\\/]/).filter(Boolean).at(-1) ?? 'Samples';
      dialogMode = 'source';
      dialog.showModal();
    } catch (e) {
      fail(e);
    }
  }
  async function addSource() {
    dialog.close();
    busy = 'Scanning source…';
    try {
      await invoke('add_source', {
        path: sourcePath,
        label: sourceLabel,
        storage,
      });
      await refreshStatus();
      await load();
    } catch (e) {
      fail(e);
    } finally {
      busy = '';
    }
  }
  async function scanSource(id: number) {
    busy = 'Scanning source…';
    try {
      await invoke('rescan', { id });
      await refreshStatus();
      await load(offset);
    } catch (e) {
      fail(e);
    } finally {
      busy = '';
    }
  }
  async function analyze() {
    try {
      await invoke('start_analysis');
      analyzing = true;
    } catch (e) {
      fail(e);
    }
  }
  function openTag() {
    if (!selection.length) return;
    tagName = '';
    dialogMode = 'tag';
    dialog.showModal();
  }
  async function saveTag() {
    if (!tagName.trim()) return;
    dialog.close();
    try {
      await invoke('edit_tag', {
        ids: selection,
        name: tagName.trim().replace(/^#/, ''),
        remove: false,
      });
      await refreshStatus();
      await load(offset);
    } catch (e) {
      fail(e);
    }
  }
  async function removeTag(name: string) {
    if (!selected) return;
    const id = selected.id;
    try {
      await invoke('edit_tag', { ids: [id], name, remove: true });
      await refreshStatus();
      await load(offset);
      if (selected?.id === id)
        selected = {
          ...selected,
          tags: selected.tags.filter((t) => t !== name),
        };
    } catch (e) {
      fail(e);
    }
  }
  async function saveMetadata() {
    if (!selected) return;
    try {
      await invoke('edit_metadata', {
        id: selected.id,
        bpm: metaBpm ? Number(metaBpm) : null,
        key: metaKey !== '' ? Number(metaKey) : null,
        mode: metaMode || null,
      });
      await load(offset);
    } catch (e) {
      fail(e);
    }
  }
  function chooseCollection(collection: Collection) {
    activeCollection =
      activeCollection === collection.id ? null : collection.id;
    activeMap = null;
    activeRoot = null;
    tagFilter = null;
    query = '';
    similarityAnchor = null;
    selection = [];
    mode = 'list';
    if (scroller) scroller.scrollTop = 0;
    void load();
  }
  async function organized(deleted?: number) {
    if (activeCollection === deleted) activeCollection = null;
    await refreshStatus();
    await load(offset);
    if (selected) {
      const page = await invoke<Page>('sample', { id: selected.id });
      if (page.items[0]) selected = page.items[0];
    }
  }
  async function removeCollectionItems() {
    try {
      await invoke('edit_collection_items', {
        id: activeCollection,
        ids: selection,
        remove: true,
      });
      selection = [];
      await refreshStatus();
      await load();
    } catch (e) {
      fail(e);
    }
  }
  async function moveCollectionItem(earlier: boolean) {
    if (!selected) return;
    try {
      similarityAnchor = null;
      await invoke('move_collection_item', {
        id: activeCollection,
        sample: selected.id,
        earlier,
      });
      await load(offset);
    } catch (e) {
      fail(e);
    }
  }
  function dragImage(name: string) {
    const canvas = document.createElement('canvas');
    canvas.width = 280;
    canvas.height = 48;
    const ctx = canvas.getContext('2d')!;
    ctx.fillStyle = '#24282d';
    ctx.fillRect(0, 0, 280, 48);
    ctx.fillStyle = '#ffbd47';
    ctx.beginPath();
    ctx.arc(20, 24, 5, 0, Math.PI * 2);
    ctx.fill();
    ctx.fillStyle = '#f0f1f3';
    ctx.font = '13px monospace';
    ctx.fillText(name.slice(0, 27), 36, 29);
    return canvas.toDataURL('image/png');
  }
  async function dragFile(sample: Sample, slice = false) {
    if (!sample.available) return;
    try {
      const path = slice
        ? slicePath
        : await invoke<string>('prepare_drag', {
            id: sample.id,
            start: null,
            end: null,
          });
      if (path)
        await startDrag({
          item: [path],
          icon: dragImage(sample.name),
          mode: 'copy',
        });
    } catch (e) {
      fail(e);
    }
  }
  function beginDrag(event: PointerEvent, sample: Sample, slice = false) {
    if (event.button !== 0 || !sample.available || (slice && !slicePath))
      return;
    const x = event.clientX,
      y = event.clientY;
    const cleanup = () => {
      window.removeEventListener('pointermove', move);
      window.removeEventListener('pointerup', cleanup);
      window.removeEventListener('pointercancel', cleanup);
    };
    const move = (e: PointerEvent) => {
      if (Math.hypot(e.clientX - x, e.clientY - y) > 5) {
        cleanup();
        void dragFile(sample, slice);
      }
    };
    window.addEventListener('pointermove', move);
    window.addEventListener('pointerup', cleanup, { once: true });
    window.addEventListener('pointercancel', cleanup, { once: true });
  }
  async function prepareSlice(snap = true) {
    if (!selected || !wave) return;
    const revision = ++sliceRequest;
    const id = selected.id;
    preparing = true;
    slicePath = null;
    try {
      const [start, end] = snap
        ? await invoke<[number, number]>('snap_slice', {
            id,
            start: sliceStart,
            end: sliceEnd,
          })
        : [sliceStart, sliceEnd];
      if (revision !== sliceRequest) return;
      sliceStart = start;
      sliceEnd = end;
      const path = await invoke<string>('prepare_drag', { id, start, end });
      if (revision === sliceRequest) slicePath = path;
    } catch (e) {
      if (revision === sliceRequest) fail(e);
    } finally {
      if (revision === sliceRequest) preparing = false;
    }
  }
  function moveHandle(event: PointerEvent, edge: 'start' | 'end') {
    if (!wave) return;
    event.preventDefault();
    event.stopPropagation();
    ++sliceRequest;
    slicePath = null;
    const target = event.currentTarget as HTMLElement;
    target.focus();
    target.setPointerCapture(event.pointerId);
    const move = (e: PointerEvent) => {
      if (!wave) return;
      const rect = waveBox.getBoundingClientRect();
      const frame = Math.round(
        Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width)) *
          wave.frames,
      );
      if (edge === 'start') sliceStart = Math.min(frame, sliceEnd - 1);
      else sliceEnd = Math.max(frame, sliceStart + 1);
    };
    const end = () => {
      target.removeEventListener('pointermove', move);
      target.removeEventListener('pointerup', end);
      target.removeEventListener('pointercancel', end);
      void prepareSlice();
    };
    target.addEventListener('pointermove', move);
    target.addEventListener('pointerup', end);
    target.addEventListener('pointercancel', end);
  }
  function adjustHandle(event: KeyboardEvent, edge: 'start' | 'end') {
    if (!wave || !['ArrowLeft', 'ArrowRight'].includes(event.key)) return;
    event.preventDefault();
    const step =
      (event.altKey
        ? 1
        : Math.max(
            1,
            Math.round(wave.sample_rate * (event.shiftKey ? 0.1 : 0.01)),
          )) * (event.key === 'ArrowLeft' ? -1 : 1);
    if (edge === 'start')
      sliceStart = Math.max(0, Math.min(sliceEnd - 1, sliceStart + step));
    else
      sliceEnd = Math.min(
        wave.frames,
        Math.max(sliceStart + 1, sliceEnd + step),
      );
    void prepareSlice(!event.altKey);
  }
  async function saveSettings() {
    try {
      await invoke('configure_link', {
        enabled: !!playback.link_enabled,
        tempo: linkTempo,
      });
    } catch (e) {
      fail(e);
      return;
    }
    localStorage.setItem('sampler-audition', JSON.stringify(settings()));
    dialog.close();
  }
  onMount(() => {
    if (!isTauri()) {
      loading = false;
      error = 'Open Sampler as a desktop app to access your audio library.';
      return;
    }
    try {
      const saved = JSON.parse(
        localStorage.getItem('sampler-audition') ?? 'null',
      );
      if (saved) {
        matchLufs = saved.match_lufs ?? true;
        targetLufs = Math.max(
          -36,
          Math.min(-6, Number(saved.target_lufs) || -16),
        );
      }
    } catch {
      /* Ignore stale local audition preferences. */
    }
    void refreshStatus()
      .then(() => load())
      .catch(fail);
    let disposed = false;
    const unlisten: (() => void)[] = [];
    let lastAnalysisRefresh = 0;
    for (const event of ['library-updated', 'analysis-progress']) {
      void listen<string | null>(event, ({ payload }) => {
        if (payload) fail(payload);
        void refreshStatus().catch(fail);
        if (
          event === 'library-updated' ||
          Date.now() - lastAnalysisRefresh > 5000
        ) {
          lastAnalysisRefresh = Date.now();
          void load(offset);
        }
      }).then((off) => {
        if (disposed) off();
        else unlisten.push(off);
      });
    }
    let polling = false;
    const interval = setInterval(() => {
      if (polling) return;
      polling = true;
      void invoke<Playback>('playback_status')
        .then((status) => {
          playback = status;
          if (status.error) error = status.error;
        })
        .catch(fail)
        .finally(() => (polling = false));
    }, 60);
    return () => {
      disposed = true;
      clearInterval(interval);
      clearTimeout(searchTimer);
      clearTimeout(scrollTimer);
      unlisten.forEach((off) => off());
    };
  });
</script>

{#snippet tagTree(parent: number | null, depth = 0, prefix = '')}
  {#each tags.filter((t) => t.parent_id === parent) as tag}
    {@const path = prefix ? prefix + '/' + tag.name : tag.name}
    <button
      class="tag-row"
      class:active={activeTag === path}
      style:padding-left={`${9 + depth * 12}px`}
      onclick={() => filterTag(activeTag === path ? null : path)}
      ><span
        >{#if depth}<i
            class="dot"
            style:background={kind({ name: tag.name, tags: [] }).color}
          ></i>{/if}{tag.name}</span
      ><span class="count">{tag.count || ''}</span></button
    >
    {@render tagTree(tag.id, depth + 1, path)}
  {/each}
{/snippet}

<svelte:window onkeydown={keyboard} bind:innerWidth={viewportWidth} />
<div class="app-shell">
  <header class="topbar">
    <button
      class="brand"
      onclick={() => {
        mode = 'list';
        activeMap = null;
        activeCollection = null;
        mapInfo = null;
        mapBuffer = null;
        query = '';
        similarityAnchor = null;
        filterRoot(null);
        filterTag(null);
      }}
      aria-label="Show full library"
      ><span class="brand-mark"><i></i><i></i><i></i><i></i><i></i></span
      ><strong>Library</strong></button
    >
    <nav class="view-switch" aria-label="Library view">
      <button class:chosen={mode === 'map'} onclick={showMap}>Map</button
      ><button
        class:chosen={mode === 'list'}
        onclick={() => {
          mode = 'list';
          void load();
        }}>List</button
      >
    </nav>
    <div class="search">
      <Icon name="search" />{#if activeTag}<button
          class="search-chip"
          onclick={() => {
            query = query.replace(/(?:^|\s)#[^\s]+/g, '').trim();
            filterTag(null);
          }}>#{activeTag}<Icon name="close" size={12} /></button
        >{/if}{#if parsed.semantic}<span class="search-chip">Sound search</span
        >{/if}<input
        bind:this={searchInput}
        bind:value={query}
        oninput={searchChanged}
        placeholder="Name, #tag, or describe a sound…"
        aria-label="Search samples"
      /><kbd>⌘ K</kbd>
    </div>
    <button
      class="link-button"
      class:enabled={playback.link_enabled}
      aria-pressed={!!playback.link_enabled}
      title={`${playback.link_peers ?? 0} peers · align loop starts to a 4-beat bar`}
      onclick={() =>
        invoke('configure_link', {
          enabled: !playback.link_enabled,
          tempo: null,
        }).catch(fail)}
      ><Icon name="link" size={15} />Link
      <span>{(playback.link_tempo ?? 124).toFixed(2)}</span></button
    >
    <button
      class="match-button"
      class:enabled={matchLufs}
      aria-pressed={matchLufs}
      onclick={() => (matchLufs = !matchLufs)}>Match LUFS</button
    >
    <div class="pitch">
      <button
        aria-label="Transpose down one semitone"
        disabled={semitones <= -24}
        onclick={() => semitones--}>−</button
      ><span>{semitones > 0 ? '+' : ''}{semitones} st</span><button
        aria-label="Transpose up one semitone"
        disabled={semitones >= 24}
        onclick={() => semitones++}>+</button
      >
    </div>
    <button
      class="icon-button settings"
      aria-label="Audition settings"
      onclick={() => {
        linkTempo = playback.link_tempo ?? 124;
        dialogMode = 'settings';
        dialog.showModal();
      }}><Icon name="settings" /></button
    >
  </header>

  <aside class="sidebar" aria-label="Library filters">
    <div class="sidebar-scroll">
      <section>
        <div class="section-heading">
          <h2>Sources</h2>
          <button
            class="icon-button"
            onclick={chooseSource}
            aria-label="Add source"><Icon name="plus" size={15} /></button
          >
        </div>
        <button
          class="source-row"
          class:active={activeRoot === null}
          onclick={() => filterRoot(null)}
          ><span>All sources</span><span class="count"
            >{count(roots.reduce((n, r) => n + r.files, 0))}</span
          ></button
        >
        {#each roots as root}<div class="source-line">
            <button
              class="source-row"
              class:active={activeRoot === root.id}
              onclick={() => filterRoot(root.id)}
              title={root.path}
              ><span class="source-label"
                >{root.label}{#if root.storage !== 'local'}<small
                    >{root.storage === 'external' ? 'EXT' : 'NET'}</small
                  >{/if}{#if root.status === 'offline'}<small class="offline"
                    >OFFLINE</small
                  >{/if}</span
              ><span class="count">{count(root.files)}</span></button
            ><button
              class="source-refresh icon-button"
              aria-label={`Rescan ${root.label}`}
              title="Rescan source"
              onclick={() => scanSource(root.id)}
              disabled={!!busy || analyzing || embedding}
              ><Icon name="refresh" size={13} /></button
            >
          </div>{/each}
        {#if !roots.length}<button
            class="add-source-empty"
            onclick={chooseSource}
            ><Icon name="folder" />Add your sample folders</button
          >{/if}
      </section>
      <section>
        <div class="section-heading">
          <h2>Tags</h2>
          <button
            class="icon-button"
            disabled={!selected}
            onclick={openTag}
            aria-label="Tag selection"><Icon name="plus" size={15} /></button
          >
        </div>
        {#if !tags.length}<p class="sidebar-empty">
            Select a sound to add your first tag.
          </p>{/if}
        {@render tagTree(null)}
        <button class="manage-rules" onclick={() => organizer?.editRules()}
          ><Icon name="settings" size={13} />Tag rules</button
        >
      </section>
      <section>
        <div class="section-heading">
          <h2>Collections</h2>
          <button
            class="icon-button"
            aria-label="New collection"
            onclick={() => organizer?.editCollection()}
            ><Icon name="plus" size={15} /></button
          >
        </div>
        {#each collections as collection}<div class="source-line">
            <button
              class="source-row"
              class:active={activeCollection === collection.id}
              onclick={() => chooseCollection(collection)}
              title={collection.error ??
                (collection.kind === 'smart'
                  ? 'Smart collection'
                  : 'Saved selection')}
            >
              <span class="collection-label"
                ><Icon
                  name={collection.kind === 'smart' ? 'filter' : 'folder'}
                  size={13}
                />{collection.name}</span
              ><span class="count"
                >{collection.error ? '!' : count(collection.count)}</span
              >
            </button><button
              class="source-refresh icon-button"
              aria-label={`Edit ${collection.name}`}
              onclick={() => organizer?.editCollection(collection)}
              ><Icon name="settings" size={13} /></button
            >
          </div>{/each}
        {#if !collections.length}<p class="sidebar-empty">
            Save a selection or make a collection that follows your conditions.
          </p>{/if}
      </section>
    </div>
    <div class="embedding-status">
      <div>
        <span
          >{downloading
            ? `Downloading model · ${downloadPercent.toFixed(0)}%`
            : embedding
              ? 'Embedding sounds'
              : 'Similarity index'}</span
        ><span class="mono">{count(embedded)}</span>
      </div>
      <div class="analysis-caption">
        <span
          >{embeddingFailed
            ? `${embeddingFailed} failed · CLAP`
            : modelReady
              ? 'CLAP · on this device'
              : 'Local model · 622 MB'}</span
        >
        <button
          onclick={embedding ? pauseIndexing : indexSounds}
          disabled={downloading || analyzing || !!busy}
          >{downloading
            ? 'Downloading…'
            : embedding
              ? 'Pause'
              : modelReady
                ? 'Index sounds'
                : 'Enable'}</button
        >
      </div>
    </div>
    <div class="analysis-status">
      <div>
        <span class:analyzing
          >{busy || (analyzing ? 'Analyzing' : 'Library analysis')}</span
        ><span class="mono">{count(analysisDone)} / {count(analysisTotal)}</span
        >
      </div>
      <div class="progress-track">
        <div
          style:width={`${analysisTotal ? (analysisDone / analysisTotal) * 100 : 0}%`}
        ></div>
      </div>
      <div class="analysis-caption">
        <span
          >{analysisFailed
            ? `${analysisFailed} failed`
            : analyzing
              ? 'Loudness · key · waveforms'
              : 'Audio metadata & waveforms'}</span
        ><button
          onclick={analyze}
          disabled={embedding ||
            analyzing ||
            !!busy ||
            !analysisTotal ||
            analysisDone === analysisTotal}
          >{analyzing ? 'Running' : 'Analyze'}</button
        >
      </div>
    </div>
  </aside>

  <main class="workspace">
    {#if mode === 'list'}
      <div class="list-toolbar">
        <div class="result-count">
          <strong>{count(total)}</strong>
          samples{#if currentCollection}<span>· {currentCollection.name}</span
            >{/if}{#if activeRoot}<span
              >· {roots.find((r) => r.id === activeRoot)?.label}</span
            >{/if}{#if activeTag}<span>· #{activeTag}</span
            >{/if}{#if parsed.semantic || similarityAnchor}<span
              >· indexed sounds</span
            >{/if}{#if loading}<span class="loading-dot"></span>{/if}
        </div>
        <div class="toolbar-actions">
          <button
            class="sort-label"
            disabled={parsed.semantic ||
              !selected ||
              !embedded ||
              currentCollection?.kind === 'static'}
            onclick={() => {
              sortBySimilarity = !sortBySimilarity;
              similarityAnchor = sortBySimilarity
                ? (selected?.id ?? null)
                : null;
              if (scroller) scroller.scrollTop = 0;
              void load();
            }}
            title={similarityAnchor
              ? 'The order stays fixed while using arrow keys'
              : 'Sort by similarity to the selected sound'}
            >Sort: {parsed.semantic
              ? 'Text similarity'
              : sortBySimilarity && similarityAnchor
                ? 'Similar to selection'
                : currentCollection?.kind === 'static'
                  ? 'Collection order'
                  : 'Name ↑'}</button
          >
          <div class="column-control">
            <button
              class="outline-button"
              onclick={() => (columnsOpen = !columnsOpen)}
              >Columns <Icon name="settings" size={14} /></button
            >{#if columnsOpen}<div class="popover">
                <label
                  ><input type="checkbox" bind:checked={showTags} />Tags</label
                ><label
                  ><input type="checkbox" bind:checked={showRoot} />Root</label
                ><label
                  ><input type="checkbox" bind:checked={showBpm} />BPM</label
                >
              </div>{/if}
          </div>
        </div>
      </div>
    {/if}
    {#if mode === 'list'}
      <div class="table-header" style:grid-template-columns={columnTemplate}>
        <span>Wave</span><span>Name</span>{#if showTags}<span>Tags</span
          >{/if}{#if showRoot}<span>Root</span>{/if}<span>Length</span><span
          >LUFS</span
        >{#if showBpm}<span>BPM</span>{/if}
      </div>
      <div
        class="sample-list"
        bind:this={scroller}
        onscroll={scroll}
        role="listbox"
        tabindex="0"
        aria-label="Audio samples"
        aria-multiselectable="true"
      >
        <div class="virtual-space" style:height={`${total * rowHeight}px`}>
          {#each rows as sample, i (sample.id)}{@const color =
              kind(sample).color}<button
              role="option"
              aria-selected={selection.includes(sample.id)}
              class="sample-row"
              class:selected={selection.includes(sample.id)}
              class:playing={playback.playing &&
                playback.sample_id === sample.id}
              class:unavailable={!sample.available}
              style:top={`${(offset + i) * rowHeight}px`}
              style:grid-template-columns={columnTemplate}
              onpointerenter={() => prefetch(offset + i)}
              onfocus={() => prefetch(offset + i)}
              ondblclick={() => play(sample)}
              onclick={(e) => select(sample, offset + i, e.shiftKey)}
              onpointerdown={(e) => beginDrag(e, sample)}
              title={sample.path}
            >
              <span class="row-wave"
                ><Waveform
                  peaks={sample.peaks}
                  active={playback.playing && playback.sample_id === sample.id}
                /></span
              ><span class="sample-name"
                ><i class="dot" style:background={color}></i><span
                  >{sample.name}</span
                >{#if !sample.available}<small>OFFLINE</small>{/if}</span
              >{#if showTags}<span class="row-tags"
                  >{sample.tags.map((t) => '#' + t).join(' ') || '—'}</span
                >{/if}{#if showRoot}<span
                  >{sample.analysis?.key_root != null
                    ? keys[sample.analysis.key_root]
                    : '—'}</span
                >{/if}<span
                >{sample.analysis
                  ? decimal(sample.analysis.duration_ms / 1000, 2) + ' s'
                  : '—'}</span
              ><span>{decimal(sample.analysis?.lufs)}</span>{#if showBpm}<span
                  >{decimal(sample.analysis?.bpm)}</span
                >{/if}
            </button>{/each}
        </div>
        {#if !total && !loading}<div class="empty-state">
            <Icon name={roots.length ? 'search' : 'folder'} size={32} />
            <h2>
              {roots.length ? 'No matching sounds' : 'A home for your sounds'}
            </h2>
            <p>
              {roots.length
                ? 'Try another name, tag, or source.'
                : 'Add a sample folder to start exploring your library.'}
            </p>
            {#if !roots.length}<button
                class="primary-button"
                onclick={chooseSource}>Add source</button
              >{:else}<button
                class="outline-button"
                onclick={() => {
                  query = '';
                  similarityAnchor = null;
                  tagFilter = null;
                  activeRoot = null;
                  void load();
                }}>Clear filters</button
              >{/if}
          </div>{/if}
      </div>
    {:else}
      <div class="map-toolbar">
        <nav aria-label="Audio maps">
          {#each maps as map}<button
              class:active={activeMap === map.id}
              onclick={() => chooseMap(map.id)}>{map.name}</button
            >{/each}<button
            class="add-map"
            aria-label="Create map from current filters"
            onclick={() => {
              mapName = 'New map';
              dialogMode = 'map';
              dialog.showModal();
            }}>+</button
          >
        </nav>
        <div class="map-appearance">
          <button onclick={() => mapCanvas?.fit()}>Fit view</button>
          <label
            >Color <select aria-label="Map color" bind:value={coloring}
              ><option value={0}>Type</option><option value={1}>Key</option
              ><option value={2}>Length</option></select
            ></label
          ><button
            aria-pressed={loudnessSize}
            onclick={() => (loudnessSize = !loudnessSize)}
            >Size: {loudnessSize ? 'Loudness' : 'Uniform'}</button
          >
        </div>
      </div>
      <MapCanvas
        bind:this={mapCanvas}
        buffer={mapBuffer}
        {selection}
        playing={playback.playing ? playback.sample_id : 0}
        similar={neighbors.map((n) => n.id)}
        {coloring}
        {loudnessSize}
        labels={mapInfo?.labels ?? []}
        {traceLabel}
        seconds={playback.seconds}
        onselect={selectFromMap}
        onaudition={trace}
        onprefetch={prefetchMap}
        onlasso={mapSelection}
      />
      <div class="map-bottom">
        <div class="map-legend">
          <span>{count(total)} sounds</span>
          <span style:color="#ffbd47">● Kick</span><span style:color="#668cfa"
            >● Snare / Clap</span
          ><span style:color="#e3e7ea">● Hat</span><span style:color="#42c5ae"
            >● Percussion</span
          ><span style:color="#bf94f0">● Bass</span>
        </div>
        <button
          class="outline-button"
          disabled={!!layoutActive || !mapInfo?.points}
          onclick={recompute}
          >{layoutActive
            ? 'Recomputing…'
            : `${count(mapInfo?.provisional ?? 0)} provisional · Recompute layout`}</button
        >{#if layoutActive}<button
            class="outline-button"
            onclick={() => invoke('cancel_layout').catch(fail)}>Cancel</button
          >{/if}
      </div>
    {/if}
    <div class="list-footer">
      {#if mode === 'map'}<span
          >Hold & trace to audition · Shift+drag to lasso · Right-drag to pan</span
        >{/if}
      {#if selection.length}<span>{selection.length} selected</span><button
          onclick={openTag}>+ Tag</button
        ><button onclick={() => organizer?.addSelection()}>+ Collection</button>
        {#if currentCollection?.kind === 'static'}<button
            onclick={removeCollectionItems}>Remove from collection</button
          >
          {#if selection.length === 1 && mode === 'list'}<button
              onclick={() => moveCollectionItem(true)}
              aria-label="Move earlier in collection">↑</button
            ><button
              onclick={() => moveCollectionItem(false)}
              aria-label="Move later in collection">↓</button
            >{/if}
        {/if}
      {:else if mode === 'list'}<span
          ><kbd>↑</kbd><kbd>↓</kbd> select & play</span
        ><span>Shift+click to range-select · Drag a row to your DAW</span>{/if}
    </div>
    <div class="transport" class:empty={!selected}>
      <div class="transport-info">
        {#if selected}<div>
            <button
              class="play-control"
              aria-label={playback.playing
                ? 'Stop playback'
                : 'Play selected sample'}
              onclick={() => (playback.playing ? stop() : play())}
              ><Icon
                name={playback.playing && playback.sample_id === selected.id
                  ? 'stop'
                  : 'play'}
                size={17}
              /></button
            ><span class="transport-name" title={selected.name}
              >{selected.name}</span
            >
          </div>
          <p>
            {#if wave}Slice {(sliceStart / wave.sample_rate).toFixed(2)}–{(
                sliceEnd / wave.sample_rate
              ).toFixed(2)} / {(wave.frames / wave.sample_rate).toFixed(2)} s{:else}Loading
              waveform…{/if}
          </p>{:else}<div>
            <Icon name="volume" /><span>Select a sample</span>
          </div>
          <p>Listen. Find your sound. Make something.</p>{/if}
      </div>
      <div
        class="transport-wave"
        bind:this={waveBox}
        role="presentation"
        onpointerdown={(e) => {
          if (selected) beginDrag(e, selected);
        }}
      >
        <Waveform peaks={wave?.peaks ?? []} large />
        {#if wave && selected}<div
            class="slice-region"
            style:left={`${(sliceStart / wave.frames) * 100}%`}
            style:width={`${((sliceEnd - sliceStart) / wave.frames) * 100}%`}
          ></div>
          <button
            class="slice-handle start"
            aria-label="Slice start"
            title="← →: 10ms · Shift: 100ms · Alt: 1 frame, no snapping"
            style:left={`${(sliceStart / wave.frames) * 100}%`}
            onpointerdown={(e) => moveHandle(e, 'start')}
            onkeydown={(e) => adjustHandle(e, 'start')}><span></span></button
          ><button
            class="slice-handle end"
            aria-label="Slice end"
            title="← →: 10ms · Shift: 100ms · Alt: 1 frame, no snapping"
            style:left={`${(sliceEnd / wave.frames) * 100}%`}
            onpointerdown={(e) => moveHandle(e, 'end')}
            onkeydown={(e) => adjustHandle(e, 'end')}><span></span></button
          >{#if playback.playing && playback.sample_id === selected.id}<div
              class="playhead"
              style:left={`${progress * 100}%`}
            ></div>{/if}{/if}
      </div>
      <button
        class="drag-slice"
        disabled={!wave || preparing}
        class:ready={!!slicePath}
        onpointerdown={(e) => {
          if (selected && slicePath) beginDrag(e, selected, true);
        }}
        onclick={() => {
          if (!slicePath) void prepareSlice();
        }}
        ><Icon name="drag" size={18} /><span
          >{preparing
            ? 'Preparing…'
            : slicePath
              ? 'Drag slice'
              : 'Prepare slice'}</span
        ><small>Waveform: whole file</small></button
      >
    </div>
  </main>

  <aside class="inspector" aria-label="Sample details">
    {#if selected}<div class="inspector-kind">
        <i class="dot" style:background={selectedKind?.color}></i><span
          >{selectedKind?.name.toUpperCase()}</span
        >{#if playback.playing && playback.sample_id === selected.id}<span
            class="playing-badge"
            >{playback.waiting ? 'Waiting for bar' : 'Playing'}</span
          >{/if}
      </div>
      <h1>{selected.name}</h1>
      <p class="file-path">{selected.path}</p>
      <div class="metadata-grid">
        <div>
          <div class="field-label">Length</div>
          <span
            >{selected.analysis
              ? decimal(selected.analysis.duration_ms / 1000, 2) + ' s'
              : '—'}</span
          >
        </div>
        <div>
          <label for="root-key">Root</label><select
            id="root-key"
            bind:value={metaKey}
            onchange={saveMetadata}
            disabled={!selected.analysis}
            title={selected.analysis?.key_source ?? 'Not analyzed'}
            ><option value="">—</option>{#each keys as key, i}<option
                value={String(i)}>{key}</option
              >{/each}</select
          >
        </div>
        <div>
          <label for="bpm">BPM</label><input
            id="bpm"
            type="text"
            inputmode="decimal"
            bind:value={metaBpm}
            placeholder="—"
            onblur={saveMetadata}
            onkeydown={(e) => {
              if (e.key === 'Enter') (e.target as HTMLInputElement).blur();
            }}
            disabled={!selected.analysis}
            title={selected.analysis?.bpm_source ??
              (selected.analysis ? 'No BPM detected' : 'Not analyzed')}
          />
        </div>
        <div>
          <div class="field-label">Loudness</div>
          <span>{decimal(selected.analysis?.lufs)} <small>LUFS</small></span>
        </div>
        <div>
          <div class="field-label">Peak</div>
          <span>{decimal(selected.analysis?.peak_dbfs)} <small>dB</small></span>
        </div>
        <div>
          <div class="field-label">Format</div>
          <span
            >{wave
              ? `${(wave.sample_rate / 1000).toFixed(wave.sample_rate % 1000 ? 1 : 0)}k · ${wave.bits_per_sample ?? '—'}`
              : selected.analysis
                ? `${selected.analysis.sample_rate / 1000}k`
                : '—'}</span
          >
        </div>
      </div>
      <div class="mode-editor">
        <label for="key-mode">Key mode</label><select
          id="key-mode"
          bind:value={metaMode}
          onchange={saveMetadata}
          disabled={!selected.analysis || metaKey === ''}
          ><option value="">Root only</option><option value="major"
            >Major</option
          ><option value="minor">Minor</option></select
        >
      </div>
      <section class="inspector-tags">
        <h2>Tags</h2>
        <div class="tag-chips">
          {#each selected.tags as tag}<button
              class="tag-chip"
              title={`Remove #${tag}`}
              onclick={() => removeTag(tag)}>#{tag}<span>×</span></button
            >{/each}<button class="add-tag" onclick={openTag}>+ Tag</button>
        </div>
      </section>
      <section class="similar">
        <h2>Similar</h2>
        {#if findingSimilar}<p class="sidebar-empty">Finding nearby sounds…</p>
        {:else if similarError}<p class="sidebar-empty">{similarError}</p>
        {:else if neighbors.length}
          {#each neighbors as sample}<button
              class="similar-row"
              onclick={() => select(sample, 0)}
              title={sample.path}
              ><i class="dot" style:background={kind(sample).color}></i><span
                >{sample.name}</span
              ><small>{sample.similarity?.toFixed(2)}</small></button
            >{/each}
        {:else}<p class="sidebar-empty">
            {modelReady
              ? 'Index more sounds to find similar samples.'
              : 'Enable local similarity search to find sounds like this.'}
          </p>{/if}
      </section>
      <div class="inspector-bottom">
        <Icon name="check" size={14} /><span>Original file stays untouched</span
        >
      </div>
    {:else}<div class="inspector-placeholder">
        <Icon name="volume" size={26} />
        <h2>Every sound, a little closer.</h2>
        <p>
          Select a sample to inspect its details, edit tags, and choose a slice.
        </p>
        <div><kbd>Space</kbd> to play · <kbd>Esc</kbd> to clear</div>
      </div>{/if}
  </aside>
  {#if error}<div class="error-toast" role="alert">
      <span>{error}</span><button
        aria-label="Dismiss message"
        onclick={() => (error = '')}><Icon name="close" size={16} /></button
      >
    </div>{/if}
</div>

<dialog bind:this={dialog} onclose={() => {}}>
  <div class="dialog-heading">
    <h2>
      {dialogMode === 'tag'
        ? 'Tag selection'
        : dialogMode === 'source'
          ? 'Add sample source'
          : dialogMode === 'map'
            ? 'New audio map'
            : 'Audition settings'}
    </h2>
    <button
      class="icon-button"
      onclick={() => dialog.close()}
      aria-label="Close dialog"><Icon name="close" /></button
    >
  </div>
  {#if dialogMode === 'tag'}<form
      onsubmit={(e) => {
        e.preventDefault();
        void saveTag();
      }}
    >
      <p>
        Add a tag to {selection.length} selected {selection.length === 1
          ? 'sample'
          : 'samples'}. Use / for a hierarchy.
      </p>
      <label for="tag-name">Tag name</label><input
        id="tag-name"
        bind:value={tagName}
        placeholder="Drums/kick"
        required
      /><button class="primary-button" type="submit">Add tag</button>
    </form>
  {:else if dialogMode === 'map'}<form
      onsubmit={(e) => {
        e.preventDefault();
        void saveMap();
      }}
    >
      <p>Save the current search, source and tag filters as an audio map.</p>
      <label for="map-name">Map name</label><input
        id="map-name"
        bind:value={mapName}
        required
        maxlength="100"
      /><button class="primary-button" type="submit">Create map</button>
    </form>
  {:else if dialogMode === 'source'}<form
      onsubmit={(e) => {
        e.preventDefault();
        void addSource();
      }}
    >
      <p class="source-path">{sourcePath}</p>
      <label for="source-label">Source name</label><input
        id="source-label"
        bind:value={sourceLabel}
        required
      /><label for="storage">Storage</label><select
        id="storage"
        bind:value={storage}
        ><option value="local">Local drive</option><option value="external"
          >External drive</option
        ><option value="network">Network storage</option></select
      ><button class="primary-button" type="submit">Add & scan</button>
    </form>
  {:else}<form
      onsubmit={(e) => {
        e.preventDefault();
        void saveSettings();
      }}
    >
      <p>Match preview loudness while keeping peak headroom.</p>
      <label for="link-tempo">Link tempo (BPM)</label><input
        id="link-tempo"
        type="number"
        min="20"
        max="999"
        step="0.01"
        bind:value={linkTempo}
        required
      />
      <p class="muted">
        {playback.link_peers ?? 0} connected peers. Link aligns loop starts to a 4-beat
        bar. Playback keeps its original tempo.
      </p>
      <label for="target-lufs">Target loudness (LUFS)</label><input
        id="target-lufs"
        type="number"
        min="-36"
        max="-6"
        step="1"
        bind:value={targetLufs}
        required
      /><label class="checkbox-label"
        ><input type="checkbox" bind:checked={matchLufs} />Match LUFS when
        auditioning</label
      ><button
        class="outline-button"
        type="button"
        disabled={embedding || downloading}
        onclick={repairModel}
        >{downloading
          ? 'Checking model…'
          : 'Verify / repair similarity model'}</button
      >
      <button class="primary-button" type="submit">Save settings</button>
    </form>{/if}
</dialog>

<Organization
  bind:this={organizer}
  {collections}
  {rules}
  {roots}
  {selection}
  selectedId={selected?.id ?? null}
  currentQuery={currentMapQuery}
  ondone={organized}
/>
