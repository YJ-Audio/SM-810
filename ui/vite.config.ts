import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: ['es2022', 'safari15'],
    rolldownOptions: { input: { app: 'index.html', bench: 'bench.html' } },
  },
});
