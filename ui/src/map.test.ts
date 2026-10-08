import { describe, it, expect } from 'vitest';
import { parseMap, Grid, zoomAt, world, screen, insidePolygon } from './map';
function fixture() {
  const b = new ArrayBuffer(32 + 3 * 32),
    v = new DataView(b);
  v.setUint32(0, 0x3150414d, true);
  v.setUint32(4, 1, true);
  v.setBigInt64(8, 1n, true);
  v.setUint32(24, 3, true);
  for (let i = 0; i < 3; i++) {
    const p = 32 + i * 32;
    v.setBigInt64(p, BigInt(i + 1), true);
    v.setFloat32(p + 8, 0.5 + i * 0.01, true);
    v.setFloat32(p + 12, 0.5, true);
    v.setUint8(p + 18, i === 1 ? 0 : 3);
    v.setFloat32(p + 20, 3, true);
  }
  return b;
}
describe('map protocol and interaction geometry', () => {
  it('decodes fixed records and hits only matching available samples', () => {
    const data = parseMap(fixture()),
      grid = new Grid(data);
    expect([...data.ids]).toEqual([1, 2, 3]);
    expect(grid.nearby(0.501, 0.5, 0.02, 3)).toEqual([0, 2]);
    expect(() => parseMap(new ArrayBuffer(16))).toThrow();
  });
  it('keeps the same world point under the cursor while zooming', () => {
    const camera = { zoom: 1, panX: 30, panY: -12, width: 1000, height: 600 },
      cursor: [number, number] = [120, 450],
      before = world(cursor, camera),
      next = zoomAt(camera, cursor, 3);
    const after = screen(before, next);
    expect(after[0]).toBeCloseTo(cursor[0], 8);
    expect(after[1]).toBeCloseTo(cursor[1], 8);
  });
  it('keeps only the nearest candidates in a dense zoomed-out region', () => {
    const count = 2000;
    const data = {
      ...parseMap(fixture()),
      count,
      ids: Float64Array.from({ length: count }, (_, i) => i + 1),
      xy: Float32Array.from(
        { length: count * 2 },
        (_, i) => ((i * 31) % 1999) / 1999,
      ),
      flags: Uint8Array.from({ length: count }, (_, i) => (i % 3 ? 3 : 2)),
    };
    const expected = Array.from({ length: count }, (_, index) => ({
      index,
      distance:
        (data.xy[index * 2] - 0.5) ** 2 + (data.xy[index * 2 + 1] - 0.5) ** 2,
    }))
      .filter(
        ({ index, distance }) =>
          data.flags[index] === 3 && distance <= 0.5 ** 2,
      )
      .sort((a, b) => a.distance - b.distance || a.index - b.index)
      .slice(0, 8)
      .map((hit) => hit.index);
    expect(new Grid(data).nearby(0.5, 0.5, 0.5, 8)).toEqual(expected);
  });
  it('supports concave lasso polygons', () => {
    const shape: [number, number][] = [
      [0, 0],
      [2, 0],
      [2, 2],
      [1, 1],
      [0, 2],
    ];
    expect(insidePolygon(0.2, 0.2, shape)).toBe(true);
    expect(insidePolygon(1, 1.8, shape)).toBe(false);
  });
});
