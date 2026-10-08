import { mount } from 'svelte';
import '@fontsource/geist/400.css';
import '@fontsource/geist/500.css';
import '@fontsource/geist/600.css';
import '@fontsource/geist-mono/400.css';
import './style.css';
import App from './App.svelte';
mount(App, { target: document.getElementById('app')! });
