export type MapData = {
  id: number;
  revision: number;
  provisional: number;
  count: number;
  ids: Float64Array;
  xy: Float32Array;
  types: Uint8Array;
  keys: Uint8Array;
  flags: Uint8Array;
  sizes: Float32Array;
  durations: Float32Array;
};
export function parseMap(buffer: ArrayBuffer): MapData {
  const view = new DataView(buffer);
  if (
    buffer.byteLength < 32 ||
    view.getUint32(0, true) !== 0x3150414d ||
    view.getUint32(4, true) !== 1
  )
    throw new Error('Unsupported map data');
  const count = view.getUint32(24, true);
  if (count > 100000 || buffer.byteLength !== 32 + count * 32)
    throw new Error('Invalid map data length');
  const result: MapData = {
    id: Number(view.getBigInt64(8, true)),
    revision: Number(view.getBigInt64(16, true)),
    count,
    provisional: view.getUint32(28, true),
    ids: new Float64Array(count),
    xy: new Float32Array(count * 2),
    types: new Uint8Array(count),
    keys: new Uint8Array(count),
    flags: new Uint8Array(count),
    sizes: new Float32Array(count),
    durations: new Float32Array(count),
  };
  for (let i = 0; i < count; i++) {
    const offset = 32 + i * 32;
    result.ids[i] = Number(view.getBigInt64(offset, true));
    result.xy[i * 2] = view.getFloat32(offset + 8, true);
    result.xy[i * 2 + 1] = view.getFloat32(offset + 12, true);
    if (
      !Number.isSafeInteger(result.ids[i]) ||
      !Number.isFinite(result.xy[i * 2]) ||
      !Number.isFinite(result.xy[i * 2 + 1])
    )
      throw new Error('Invalid map coordinates');
    result.types[i] = view.getUint8(offset + 16);
    result.keys[i] = view.getUint8(offset + 17);
    result.flags[i] = view.getUint8(offset + 18);
    result.sizes[i] = view.getFloat32(offset + 20, true);
    result.durations[i] = view.getFloat32(offset + 24, true);
  }
  return result;
}
export type Camera = {
  zoom: number;
  panX: number;
  panY: number;
  width: number;
  height: number;
};
export function screen(
  point: [number, number],
  camera: Camera,
): [number, number] {
  const scale = Math.min(camera.width, camera.height) * 0.9 * camera.zoom;
  return [
    (point[0] - 0.5) * scale + camera.width / 2 + camera.panX,
    (point[1] - 0.5) * scale + camera.height / 2 + camera.panY,
  ];
}
export function world(
  point: [number, number],
  camera: Camera,
): [number, number] {
  const scale = Math.min(camera.width, camera.height) * 0.9 * camera.zoom;
  return [
    (point[0] - camera.width / 2 - camera.panX) / scale + 0.5,
    (point[1] - camera.height / 2 - camera.panY) / scale + 0.5,
  ];
}
export function zoomAt(
  camera: Camera,
  point: [number, number],
  factor: number,
): Camera {
  const position = world(point, camera);
  const result = {
    ...camera,
    zoom: Math.max(0.2, Math.min(128, camera.zoom * factor)),
  };
  const after = screen(position, result);
  result.panX += point[0] - after[0];
  result.panY += point[1] - after[1];
  return result;
}
export class Grid {
  private cells = new Map<string, number[]>();
  constructor(
    readonly data: MapData,
    readonly cell = 0.025,
  ) {
    for (let i = 0; i < data.count; i++) {
      const key = this.key(data.xy[i * 2], data.xy[i * 2 + 1]);
      const bucket = this.cells.get(key);
      if (bucket) bucket.push(i);
      else this.cells.set(key, [i]);
    }
  }
  private key(x: number, y: number) {
    return `${Math.floor(x / this.cell)},${Math.floor(y / this.cell)}`;
  }
  nearby(x: number, y: number, radius: number, count = 1): number[] {
    if (count < 1 || !Number.isFinite(radius) || radius <= 0) return [];
    const hits: { index: number; distance: number }[] = [];
    for (
      let a = Math.floor((x - radius) / this.cell);
      a <= Math.floor((x + radius) / this.cell);
      a++
    )
      for (
        let b = Math.floor((y - radius) / this.cell);
        b <= Math.floor((y + radius) / this.cell);
        b++
      ) {
        for (const index of this.cells.get(`${a},${b}`) ?? []) {
          if ((this.data.flags[index] & 3) !== 3) continue;
          const distance =
            (this.data.xy[index * 2] - x) ** 2 +
            (this.data.xy[index * 2 + 1] - y) ** 2;
          if (distance > radius * radius) continue;
          const id = this.data.ids[index];
          let at = hits.length;
          while (
            at > 0 &&
            (distance < hits[at - 1].distance ||
              (distance === hits[at - 1].distance &&
                id < this.data.ids[hits[at - 1].index]))
          )
            at--;
          if (at < count) {
            hits.splice(at, 0, { index, distance });
            if (hits.length > count) hits.pop();
          }
        }
      }
    return hits.map((h) => h.index);
  }
}
export function insidePolygon(
  x: number,
  y: number,
  polygon: [number, number][],
): boolean {
  let inside = false;
  for (let i = 0, j = polygon.length - 1; i < polygon.length; j = i++) {
    const a = polygon[i],
      b = polygon[j];
    if (
      a[1] > y !== b[1] > y &&
      x < ((b[0] - a[0]) * (y - a[1])) / (b[1] - a[1]) + a[0]
    )
      inside = !inside;
  }
  return inside;
}
const vertex = `#version 300 es
precision highp float;
layout(location=0) in vec2 current;
layout(location=1) in vec2 previous;
layout(location=2) in float kind;
layout(location=3) in float root;
layout(location=4) in float size;
layout(location=5) in float flags;
layout(location=6) in float ring;
layout(location=7) in float duration;
uniform vec2 resolution;
uniform vec2 pan;
uniform float zoom;
uniform float transition;
uniform int coloring;
uniform bool loudnessSize;
out vec2 local;
out vec3 color;
out float opacity;
out float radius;
flat out float ringKind;
vec3 hsv(float h){return .42+.5*clamp(abs(fract(vec3(h)+vec3(0.,2./3.,1./3.))*6.-3.)-1.,0.,1.);}
void main(){
  vec2 corners[4]=vec2[4](vec2(-1,-1),vec2(1,-1),vec2(-1,1),vec2(1,1));
  vec3 palette[6]=vec3[6](vec3(1.,.74,.28),vec3(.4,.55,.98),vec3(.89,.91,.92),vec3(.26,.77,.68),vec3(.75,.58,.94),vec3(.57,.61,.67));
  color=palette[int(clamp(kind,0.,5.))];
  if(coloring==1)color=root<12.?hsv(root/12.):vec3(.38);
  if(coloring==2)color=hsv(clamp(duration/12.,0.,1.)*.7);
  opacity=mod(flags,2.)>.5?1.:.15;
  if(mod(floor(flags/2.),2.)<.5)opacity*=.4;
  radius=(loudnessSize?size:3.4)*clamp(sqrt(zoom),.8,1.25);
  ringKind=ring;
  local=corners[gl_VertexID]*(radius+6.);
  vec2 center=(mix(previous,current,transition)-.5)*min(resolution.x,resolution.y)*.9*zoom+resolution*.5+pan;
  vec2 pixel=center+local;
  gl_Position=vec4(pixel/resolution*vec2(2.,-2.)+vec2(-1.,1.),0.,1.);
}`;
const fragment = `#version 300 es
precision highp float;
in vec2 local;
in vec3 color;
in float opacity;
in float radius;
flat in float ringKind;
out vec4 outputColor;
void main(){
  float d=length(local);
  float body=1.-smoothstep(radius-.7,radius+.3,d);
  bool playing=mod(floor(ringKind/4.),2.)>.5;
  bool selected=mod(floor(ringKind/2.),2.)>.5;
  bool similar=mod(ringKind,2.)>.5;
  float outer=selected?(1.-smoothstep(.6,1.4,abs(d-radius-(playing?4.:1.8)))):0.;
  float inner=(playing||similar)?(1.-smoothstep(.4,playing?1.4:.9,abs(d-radius-1.8))):0.;
  if(!playing)inner*=.6;
  float alpha=max(body,max(inner,outer))*opacity;
  if(alpha<.01)discard;
  vec3 result=mix(color,playing?vec3(1.,.41,.24):vec3(.95),inner);
  outputColor=vec4(mix(result,vec3(.95),outer),alpha);
}`;
export class Renderer {
  readonly gl: WebGL2RenderingContext;
  private program: WebGLProgram;
  private buffer: WebGLBuffer;
  private vao: WebGLVertexArrayObject;
  private locations: Record<string, WebGLUniformLocation | null> = {};
  private data: MapData | null = null;
  private instances = new Float32Array();
  private started = 0;
  private duration = 0;
  constructor(readonly canvas: HTMLCanvasElement) {
    const gl = canvas.getContext('webgl2', {
      antialias: false,
      alpha: false,
      powerPreference: 'high-performance',
    });
    if (!gl) throw new Error('WebGL2 is required for audio maps.');
    this.gl = gl;
    const shader = (type: number, source: string) => {
      const result = gl.createShader(type)!;
      gl.shaderSource(result, source);
      gl.compileShader(result);
      if (!gl.getShaderParameter(result, gl.COMPILE_STATUS))
        throw new Error(gl.getShaderInfoLog(result) ?? 'Map shader failed');
      return result;
    };
    this.program = gl.createProgram()!;
    const vs = shader(gl.VERTEX_SHADER, vertex),
      fs = shader(gl.FRAGMENT_SHADER, fragment);
    gl.attachShader(this.program, vs);
    gl.attachShader(this.program, fs);
    gl.linkProgram(this.program);
    gl.deleteShader(vs);
    gl.deleteShader(fs);
    if (!gl.getProgramParameter(this.program, gl.LINK_STATUS))
      throw new Error(
        gl.getProgramInfoLog(this.program) ?? 'Map program failed',
      );
    this.buffer = gl.createBuffer()!;
    this.vao = gl.createVertexArray()!;
    gl.bindVertexArray(this.vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, this.buffer);
    for (const [index, size, offset] of [
      [0, 2, 0],
      [1, 2, 8],
      [2, 1, 16],
      [3, 1, 20],
      [4, 1, 24],
      [5, 1, 28],
      [6, 1, 32],
      [7, 1, 36],
    ]) {
      gl.enableVertexAttribArray(index);
      gl.vertexAttribPointer(index, size, gl.FLOAT, false, 40, offset);
      gl.vertexAttribDivisor(index, 1);
    }
    for (const name of [
      'resolution',
      'pan',
      'zoom',
      'transition',
      'coloring',
      'loudnessSize',
    ])
      this.locations[name] = gl.getUniformLocation(this.program, name);
    gl.enable(gl.BLEND);
    gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
  }
  update(data: MapData) {
    const old = this.data;
    const animate = old?.id === data.id && old.revision !== data.revision;
    const positions = new Map<number, number>();
    if (animate && old)
      for (let i = 0; i < old.count; i++) positions.set(old.ids[i], i);
    this.instances = new Float32Array(data.count * 10);
    for (let i = 0; i < data.count; i++) {
      const p = i * 10,
        previous = positions.get(data.ids[i]);
      this.instances.set(
        [
          data.xy[i * 2],
          data.xy[i * 2 + 1],
          previous !== undefined && old ? old.xy[previous * 2] : data.xy[i * 2],
          previous !== undefined && old
            ? old.xy[previous * 2 + 1]
            : data.xy[i * 2 + 1],
          data.types[i],
          data.keys[i],
          data.sizes[i],
          data.flags[i],
          0,
          data.durations[i],
        ],
        p,
      );
    }
    this.data = data;
    this.started = performance.now();
    this.duration = animate ? 650 : 0;
    this.upload();
  }
  rings(selected: number[], playing: number, similar: number[]) {
    if (!this.data) return;
    const selection = new Set(selected),
      neighbors = new Set(similar);
    for (let i = 0; i < this.data.count; i++) {
      const id = this.data.ids[i];
      this.instances[i * 10 + 8] =
        (id === playing ? 4 : 0) |
        (selection.has(id) ? 2 : 0) |
        (neighbors.has(id) ? 1 : 0);
    }
    this.upload();
  }
  private upload() {
    this.gl.bindBuffer(this.gl.ARRAY_BUFFER, this.buffer);
    this.gl.bufferData(
      this.gl.ARRAY_BUFFER,
      this.instances,
      this.gl.DYNAMIC_DRAW,
    );
  }
  get transitioning() {
    return (
      this.duration > 0 && performance.now() < this.started + this.duration
    );
  }
  draw(
    camera: Camera,
    coloring = 0,
    loudnessSize = true,
    time = performance.now(),
  ) {
    const gl = this.gl,
      dpr = Math.min(window.devicePixelRatio || 1, 2);
    const width = Math.round(camera.width * dpr),
      height = Math.round(camera.height * dpr);
    if (this.canvas.width !== width || this.canvas.height !== height) {
      this.canvas.width = width;
      this.canvas.height = height;
    }
    gl.viewport(0, 0, width, height);
    gl.clearColor(0.051, 0.059, 0.063, 1);
    gl.clear(gl.COLOR_BUFFER_BIT);
    if (!this.data) return;
    gl.useProgram(this.program);
    gl.bindVertexArray(this.vao);
    gl.uniform2f(this.locations.resolution, camera.width, camera.height);
    gl.uniform2f(this.locations.pan, camera.panX, camera.panY);
    gl.uniform1f(this.locations.zoom, camera.zoom);
    gl.uniform1f(
      this.locations.transition,
      this.duration ? Math.min(1, (time - this.started) / this.duration) : 1,
    );
    gl.uniform1i(this.locations.coloring, coloring);
    gl.uniform1i(this.locations.loudnessSize, loudnessSize ? 1 : 0);
    gl.drawArraysInstanced(gl.TRIANGLE_STRIP, 0, 4, this.data.count);
  }
  dispose() {
    this.gl.deleteBuffer(this.buffer);
    this.gl.deleteVertexArray(this.vao);
    this.gl.deleteProgram(this.program);
  }
}
