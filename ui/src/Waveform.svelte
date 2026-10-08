<script lang="ts">
  import { waveformPath } from './types';
  let {
    peaks,
    active = false,
    large = false,
  }: { peaks: number[]; active?: boolean; large?: boolean } = $props();
  let path = $derived(
    waveformPath(peaks, large ? 1024 : 160, large ? 100 : 32),
  );
</script>

<svg
  class:active
  class:large
  viewBox={large ? '0 0 1024 100' : '0 0 160 32'}
  preserveAspectRatio="none"
  aria-label={peaks.length ? 'Audio waveform' : 'Waveform awaiting analysis'}
  role="img"
>
  {#if peaks.length}<path d={path} />{:else}<path
      class="pending"
      d={large ? 'M0 50H1024' : 'M0 16H160'}
    />{/if}
</svg>

<style>
  svg {
    width: 100%;
    height: 28px;
    display: block;
    color: #646c78;
  }
  svg.active {
    color: var(--accent);
  }
  svg.large {
    height: 100%;
    color: #98a0aa;
  }
  path {
    stroke: currentColor;
    stroke-width: 1;
    vector-effect: non-scaling-stroke;
  }
  .pending {
    stroke-dasharray: 2 3;
    opacity: 0.5;
  }
</style>
