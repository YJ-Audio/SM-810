export type Analysis = {
  duration_ms: number;
  sample_rate: number;
  channels: number;
  lufs: number | null;
  peak_dbfs: number | null;
  bpm: number | null;
  bpm_source: string | null;
  key_root: number | null;
  key_mode: string | null;
  key_source: string | null;
  is_loop: boolean | null;
};
export type Sample = {
  id: number;
  name: string;
  path: string;
  root_id: number;
  available: boolean;
  tags: string[];
  size: number;
  analysis: Analysis | null;
  peaks: number[];
};
export type Page = { items: Sample[]; total: number };
export type Root = {
  id: number;
  path: string;
  label: string;
  storage: string;
  status: string;
  files: number;
  missing: number;
};
export type Tag = {
  id: number;
  parent_id: number | null;
  name: string;
  count: number;
};
export type Job = { kind: string; state: string; count: number };
export type Bootstrap = {
  roots: Root[];
  tags: Tag[];
  jobs: Job[];
  audio_error: string | null;
  analyzing: boolean;
};
export type Wave = {
  peaks: number[];
  frames: number;
  sample_rate: number;
  channels: number;
  bits_per_sample: number | null;
};
export type Playback = {
  sample_id: number;
  seconds: number;
  playing: boolean;
  error: string | null;
};
export const keys = [
  'C',
  'C♯',
  'D',
  'D♯',
  'E',
  'F',
  'F♯',
  'G',
  'G♯',
  'A',
  'A♯',
  'B',
];
export function kind(sample: Pick<Sample, 'name' | 'tags'>): {
  name: string;
  color: string;
} {
  const text = [sample.name, ...sample.tags].join(' ').toLowerCase();
  if (/kick|kck|\bbd\b/.test(text)) return { name: 'Kick', color: '#ffbd47' };
  if (/snare|clap|snr/.test(text))
    return { name: 'Snare / Clap', color: '#668cfa' };
  if (/hat|cymbal|ride/.test(text)) return { name: 'Hat', color: '#e3e7ea' };
  if (/perc|shaker|tom|conga/.test(text))
    return { name: 'Percussion', color: '#42c5ae' };
  if (/bass|sub/.test(text)) return { name: 'Bass', color: '#bf94f0' };
  return { name: 'Sample', color: '#929cab' };
}
export function parseSearch(input: string): {
  text: string;
  tag: string | null;
  semantic: boolean;
} {
  const tag = input.match(/(?:^|\s)#([^\s]+)/);
  const text = input.replace(/(?:^|\s)#[^\s]+/g, ' ').trim();
  return { text, tag: tag?.[1] ?? null, semantic: text.startsWith('~') };
}
export function waveformPath(
  peaks: number[],
  width = 1000,
  height = 100,
): string {
  const count = Math.floor(peaks.length / 2);
  return Array.from({ length: count }, (_, i) => {
    const x = ((i + 0.5) * width) / count;
    return `M${x.toFixed(2)},${(height / 2 - (peaks[i * 2 + 1] / 127) * height * 0.45).toFixed(2)}V${(height / 2 - (peaks[i * 2] / 127) * height * 0.45).toFixed(2)}`;
  }).join('');
}
