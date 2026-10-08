import { Renderer, Grid, world } from './map';
import type { MapData, Camera } from './map';
const count = 100000;
const data: MapData = {
  id: 1,
  revision: 1,
  provisional: 0,
  count,
  ids: new Float64Array(count),
  xy: new Float32Array(count * 2),
  types: new Uint8Array(count),
  keys: new Uint8Array(count),
  flags: new Uint8Array(count),
  sizes: new Float32Array(count),
  durations: new Float32Array(count),
};
let random = 810;
const next = () => {
  random ^= random << 13;
  random ^= random >>> 17;
  random ^= random << 5;
  return (random >>> 0) / 4294967296;
};
for (let i = 0; i < count; i++) {
  const cluster = i % 6,
    angle = next() * Math.PI * 2,
    radius = Math.sqrt(next()) * 0.13;
  data.ids[i] = i + 1;
  data.xy[i * 2] = 0.2 + (cluster % 3) * 0.3 + Math.cos(angle) * radius;
  data.xy[i * 2 + 1] =
    0.3 + Math.floor(cluster / 3) * 0.4 + Math.sin(angle) * radius;
  data.types[i] = cluster;
  data.keys[i] = i % 13;
  data.flags[i] = i % 5 === 0 ? 2 : 3;
  data.sizes[i] = 2.5 + next() * 3;
  data.durations[i] = next() * 10;
}
const result = document.querySelector<HTMLDivElement>('#result')!,
  canvas = document.querySelector<HTMLCanvasElement>('#canvas')!;
try {
  const renderer = new Renderer(canvas),
    grid = new Grid(data);
  renderer.update(data);
  const camera: Camera = {
    zoom: 1,
    panX: 0,
    panY: 0,
    width: window.innerWidth,
    height: window.innerHeight,
  };
  const intervals: number[] = [],
    cpu: number[] = [];
  let previous = 0,
    frames = 0;
  function draw(time: number) {
    const begin = performance.now();
    camera.zoom = 0.2 + Math.sin(time * 0.0008) ** 2 * 0.8;
    camera.panX = Math.sin(time * 0.0007) * 50;
    camera.panY = Math.cos(time * 0.0006) * 30;
    const pointer: [number, number] = [
      camera.width / 2 + Math.sin(time * 0.002) * 200,
      camera.height / 2 + Math.cos(time * 0.001) * 140,
    ];
    const scale = Math.min(camera.width, camera.height) * 0.9 * camera.zoom;
    const hit = grid.nearby(...world(pointer, camera), 12 / scale, 1);
    if (frames % 6 === 0) {
      const nearby = grid.nearby(...world(pointer, camera), 80 / scale, 8);
      renderer.rings(
        [1],
        hit[0] === undefined ? 0 : data.ids[hit[0]],
        nearby.slice(1, 6).map((i) => data.ids[i]),
      );
    }
    renderer.draw(camera, 0, true, time);
    if (frames >= 60) {
      intervals.push(time - previous);
      cpu.push(performance.now() - begin);
    }
    previous = time;
    frames++;
    if (frames < 660) {
      if (frames % 60 === 0)
        result.textContent = `Measuring ${Math.min(600, Math.max(0, frames - 60))} / 600 frames`;
      requestAnimationFrame(draw);
    } else {
      const sorted = [...intervals].sort((a, b) => a - b);
      const fps =
        1000 / (intervals.reduce((a, b) => a + b, 0) / intervals.length);
      const summary = {
        points: count,
        frames: intervals.length,
        fps,
        p95_interval_ms: sorted[Math.floor(sorted.length * 0.95)],
        over_25ms: intervals.filter((n) => n > 25).length,
        mean_cpu_ms: cpu.reduce((a, b) => a + b, 0) / cpu.length,
        viewport: [camera.width, camera.height],
        dpr: window.devicePixelRatio,
        renderer: renderer.gl.getParameter(renderer.gl.RENDERER),
        userAgent: navigator.userAgent,
      };
      result.textContent = JSON.stringify(summary, null, 2);
      result.style.whiteSpace = 'pre-wrap';
    }
  }
  requestAnimationFrame(draw);
} catch (error) {
  result.textContent = String(error);
}
