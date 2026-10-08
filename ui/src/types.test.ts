import { describe, it, expect } from 'vitest';
import { parseSearch, waveformPath } from './types';
describe('search syntax', () => {
  it('separates a tag from literal filename text', () => {
    expect(parseSearch('#dark kick')).toEqual({
      tag: 'dark',
      text: 'kick',
      semantic: false,
    });
  });
  it('preserves natural language queries for the semantic backend', () => {
    expect(parseSearch('~ a warm kick')).toEqual({
      tag: null,
      text: '~ a warm kick',
      semantic: true,
    });
  });
  it('accepts hierarchical tags', () => {
    expect(parseSearch('#Drums/kick dry')).toEqual({
      tag: 'Drums/kick',
      text: 'dry',
      semantic: false,
    });
  });
});
describe('waveform geometry', () => {
  it('keeps signed minimum and maximum around the midpoint', () => {
    expect(waveformPath([-127, 127], 100, 100)).toBe('M50.00,5.00V95.00');
  });
  it('does not invent a waveform for pending samples', () =>
    expect(waveformPath([])).toBe(''));
});
