<script lang="ts">
  import { onMount } from 'svelte';
  import {
    Renderer,
    Grid,
    parseMap,
    world,
    screen,
    zoomAt,
    insidePolygon,
  } from './map';
  import type { Camera, MapData } from './map';
  export type MapLabel = { x: number; y: number; text: string };
  let {
    buffer,
    selection = [],
    playing = 0,
    similar = [],
    coloring = 0,
    loudnessSize = true,
    labels = [],
    traceLabel = '',
    seconds = 0,
    onselect,
    onaudition,
    onprefetch,
    onlasso,
  }: {
    buffer: ArrayBuffer | null;
    selection?: number[];
    playing?: number;
    similar?: number[];
    coloring?: number;
    loudnessSize?: boolean;
    labels?: MapLabel[];
    traceLabel?: string;
    seconds?: number;
    onselect: (id: number) => void;
    onaudition: (id: number | null) => void;
    onprefetch: (ids: number[]) => void;
    onlasso: (ids: number[]) => void;
  } = $props();
  let canvas: HTMLCanvasElement, container: HTMLDivElement;
  let renderer = $state<Renderer | null>(null),
    data = $state.raw<MapData | null>(null),
    grid: Grid | null = null;
  let camera = $state<Camera>({
    zoom: 1,
    panX: 0,
    panY: 0,
    width: 800,
    height: 600,
  });
  let error = $state(''),
    gesture = $state<'scrub' | 'pan' | 'lasso' | null>(null),
    polygon = $state<[number, number][]>([]),
    cursor = $state<[number, number]>([0, 0]),
    cursorVisible = $state(false);
  let last: [number, number] = [0, 0],
    lastId: number | null = null,
    stoppedOutside = false,
    lastPrefetch = 0,
    frame = 0;
  function draw() {
    if (frame) return;
    frame = requestAnimationFrame(function paint(time) {
      frame = 0;
      renderer?.draw(camera, coloring, loudnessSize, time);
      if (renderer?.transitioning) draw();
    });
  }
  export function fit() {
    if (!data?.count) return;
    let minX = Infinity,
      maxX = -Infinity,
      minY = Infinity,
      maxY = -Infinity;
    for (let i = 0; i < data.count; i++) {
      minX = Math.min(minX, data.xy[i * 2]);
      maxX = Math.max(maxX, data.xy[i * 2]);
      minY = Math.min(minY, data.xy[i * 2 + 1]);
      maxY = Math.max(maxY, data.xy[i * 2 + 1]);
    }
    const unit = Math.min(camera.width, camera.height) * 0.9;
    camera.zoom = Math.min(
      128,
      Math.max(
        0.2,
        Math.min(
          (camera.width * 0.8) / (unit * Math.max(maxX - minX, 0.001)),
          (camera.height * 0.8) / (unit * Math.max(maxY - minY, 0.001)),
        ),
      ),
    );
    camera.panX = -(0.5 * (minX + maxX) - 0.5) * unit * camera.zoom;
    camera.panY = -(0.5 * (minY + maxY) - 0.5) * unit * camera.zoom;
  }
  $effect(() => {
    if (buffer && renderer) {
      try {
        const next = parseMap(buffer);
        data = next;
        grid = new Grid(next);
        renderer.update(next);
        renderer.rings(selection, playing, similar);
        draw();
      } catch (e) {
        error = String(e);
      }
    }
  });
  $effect(() => {
    renderer?.rings(selection, playing, similar);
    draw();
  });
  $effect(() => {
    void camera.zoom;
    void camera.panX;
    void camera.panY;
    void camera.width;
    void camera.height;
    void coloring;
    void loudnessSize;
    draw();
  });
  const local = (event: PointerEvent | WheelEvent): [number, number] => {
    const rect = canvas.getBoundingClientRect();
    return [event.clientX - rect.left, event.clientY - rect.top];
  };
  function audition(point: [number, number]) {
    if (!data || !grid || renderer?.transitioning) return;
    const location = world(point, camera),
      scale = Math.min(camera.width, camera.height) * 0.9 * camera.zoom;
    const hit = grid.nearby(...location, 12 / scale)[0];
    const id = hit === undefined ? null : data.ids[hit];
    if (id !== null && (id !== lastId || stoppedOutside)) {
      lastId = id;
      stoppedOutside = false;
      onaudition(id);
    }
    if (Date.now() - lastPrefetch > 100) {
      lastPrefetch = Date.now();
      onprefetch(
        grid.nearby(...location, 80 / scale, 8).map((i) => data!.ids[i]),
      );
    }
  }
  function down(event: PointerEvent) {
    if (error || !data) return;
    event.preventDefault();
    canvas.focus();
    canvas.setPointerCapture(event.pointerId);
    last = local(event);
    cursor = last;
    if (event.button === 1 || event.button === 2) {
      gesture = 'pan';
      return;
    }
    if (event.button !== 0) return;
    if (event.shiftKey) {
      gesture = 'lasso';
      polygon = [last];
      return;
    }
    gesture = 'scrub';
    lastId = null;
    stoppedOutside = false;
    cursorVisible = true;
    audition(last);
  }
  function move(event: PointerEvent) {
    const point = local(event);
    cursor = point;
    if (gesture === 'pan') {
      camera.panX += point[0] - last[0];
      camera.panY += point[1] - last[1];
    }
    if (
      gesture === 'lasso' &&
      Math.hypot(point[0] - last[0], point[1] - last[1]) > 3
    ) {
      polygon = [...polygon, point];
      last = point;
      return;
    }
    if (
      gesture === 'scrub' &&
      point[0] >= 0 &&
      point[1] >= 0 &&
      point[0] <= camera.width &&
      point[1] <= camera.height
    ) {
      cursorVisible = true;
      audition(point);
    } else if (gesture === 'scrub') {
      leave();
    }
    last = point;
  }
  function up(event: PointerEvent) {
    if (gesture === 'scrub' && lastId !== null) onselect(lastId);
    if (gesture === 'lasso' && polygon.length > 2 && data) {
      const shape = polygon.map((p) => world(p, camera));
      const minX = Math.min(...shape.map((p) => p[0])),
        maxX = Math.max(...shape.map((p) => p[0])),
        minY = Math.min(...shape.map((p) => p[1])),
        maxY = Math.max(...shape.map((p) => p[1]));
      const ids: number[] = [];
      for (let i = 0; i < data.count; i++) {
        const x = data.xy[i * 2],
          y = data.xy[i * 2 + 1];
        if (
          data.flags[i] & 1 &&
          x >= minX &&
          x <= maxX &&
          y >= minY &&
          y <= maxY &&
          insidePolygon(x, y, shape)
        )
          ids.push(data.ids[i]);
      }
      onlasso(ids);
    }
    if (canvas.hasPointerCapture(event.pointerId))
      canvas.releasePointerCapture(event.pointerId);
    gesture = null;
    polygon = [];
    cursorVisible = false;
  }
  function leave() {
    if (gesture === 'scrub' && !stoppedOutside) {
      onaudition(null);
      cursorVisible = false;
      stoppedOutside = true;
    }
  }
  function cancel(event: PointerEvent) {
    if (gesture === 'scrub') onaudition(null);
    lastId = null;
    gesture = null;
    polygon = [];
    cursorVisible = false;
    if (canvas.hasPointerCapture(event.pointerId))
      canvas.releasePointerCapture(event.pointerId);
  }
  function wheel(event: WheelEvent) {
    event.preventDefault();
    const point = local(event);
    if (
      event.ctrlKey ||
      event.deltaMode !== 0 ||
      (event.deltaX === 0 && Math.abs(event.deltaY) >= 50)
    )
      camera = zoomAt(camera, point, Math.exp(-event.deltaY * 0.003));
    else {
      camera.panX -= event.deltaX;
      camera.panY -= event.deltaY;
    }
  }
  onMount(() => {
    try {
      renderer = new Renderer(canvas);
    } catch (e) {
      error = String(e);
    }
    const observer = new ResizeObserver((entries) => {
      const rect = entries[0].contentRect;
      camera.width = rect.width;
      camera.height = rect.height;
    });
    observer.observe(container);
    canvas.addEventListener('wheel', wheel, { passive: false });
    const lost = (event: Event) => {
      event.preventDefault();
      onaudition(null);
      error = 'The graphics device was reset. Reopen Map view to continue.';
    };
    canvas.addEventListener('webglcontextlost', lost);
    return () => {
      observer.disconnect();
      cancelAnimationFrame(frame);
      canvas.removeEventListener('wheel', wheel);
      canvas.removeEventListener('webglcontextlost', lost);
      renderer?.dispose();
    };
  });
</script>

<div class="map-canvas" bind:this={container}>
  <canvas
    bind:this={canvas}
    aria-label="Sample similarity map. Hold and move to audition; Shift drag to select; wheel to zoom; right drag to pan."
    tabindex="0"
    onpointerdown={down}
    onpointermove={move}
    onpointerup={up}
    onpointercancel={cancel}
    onpointerleave={leave}
    oncontextmenu={(e) => e.preventDefault()}
  ></canvas>
  {#if camera.zoom < 1.7}{#each labels as label}{@const position = screen(
        [label.x, label.y],
        camera,
      )}<span
        class="map-cluster-label"
        style:left={`${position[0]}px`}
        style:top={`${position[1]}px`}>{label.text}</span
      >{/each}{/if}
  {#if gesture === 'lasso'}<svg
      class="lasso-overlay"
      width={camera.width}
      height={camera.height}
      aria-hidden="true"
      ><polygon points={polygon.map((p) => p.join(',')).join(' ')} /></svg
    >{/if}
  {#if cursorVisible && traceLabel}<div
      class="trace-label"
      style:left={`${Math.min(cursor[0] + 16, camera.width - 180)}px`}
      style:top={`${cursor[1] + 18}px`}
    >
      {traceLabel}<small>{seconds.toFixed(2)} s</small>
    </div>{/if}
  {#if error}<div class="map-message" role="alert">
      {error}
    </div>{:else if data && !data.count}<div class="map-message">
      No indexed sounds in this map.<small
        >Index sounds or choose another map.</small
      >
    </div>{/if}
</div>
