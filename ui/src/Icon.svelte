<script lang="ts">
  let { name, size = 18 }: { name: string; size?: number } = $props();
  const paths: Record<string, string> = {
    search: 'm21 21-5-5 M17 10a7 7 0 1 1-14 0 7 7 0 0 1 14 0',
    plus: 'M12 5v14 M5 12h14',
    minus: 'M5 12h14',
    close: 'm6 6 12 12 M6 18 18 6',
    folder: 'M3 7V5a1 1 0 0 1 1-1h5l2 3h9a1 1 0 0 1 1 1v11H3Z',
    settings: 'M4 7h16 M4 17h16 M8 4v6 M16 14v6',
    play: 'm8 5 11 7-11 7Z',
    stop: 'M6 6h12v12H6Z',
    arrow: 'm9 5 7 7-7 7',
    check: 'm5 12 4 4L19 6',
    refresh: 'M20 10a8 8 0 1 0 0 5 M20 4v6h-6',
    collection: 'M5 4h14v16H5Z M8 8h8 M8 12h5',
    link: 'm10 13 4-4 M8 15l-1 1a4 4 0 0 1-6-6l4-4a4 4 0 0 1 6 0 M16 9l1-1a4 4 0 0 1 6 6l-4 4a4 4 0 0 1-6 0',
    volume: 'M3 9h4l5-5v16l-5-5H3Z M16 8a6 6 0 0 1 0 8 M19 5a10 10 0 0 1 0 14',
    grid: 'M4 4h5v5H4Z M15 4h5v5h-5Z M4 15h5v5H4Z M15 15h5v5h-5Z',
    drag: 'M9 5h.01 M15 5h.01 M9 10h.01 M15 10h.01 M9 15h.01 M15 15h.01 M9 20h.01 M15 20h.01',
  };
</script>

<svg
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width={name === 'drag' ? 3 : 1.6}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"><path d={paths[name] ?? paths.grid} /></svg
>
