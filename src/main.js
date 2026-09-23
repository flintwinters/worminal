import { Terminal } from '@xterm/xterm';
import { FitAddon } from '@xterm/addon-fit';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import './style.css';

const terminalElement = document.getElementById('terminal');
const heightControl = document.getElementById('line-height');
const heightValue = document.getElementById('line-height-value');
const settings = document.getElementById('settings');
const savedHeight = Number(localStorage.getItem('lineHeight'));
const lineHeight = Number.isFinite(savedHeight) && savedHeight >= 0.5 && savedHeight <= 1.5 ? savedHeight : 1;

const term = new Terminal({
  cursorBlink: true,
  fontFamily: 'monospace',
  fontSize: 14,
  lineHeight,
  scrollback: 10000,
});
const fit = new FitAddon();
term.loadAddon(fit);
term.open(terminalElement);
term.focus();
fit.fit();

heightControl.value = String(lineHeight);
heightValue.value = lineHeight.toFixed(2);
heightControl.addEventListener('input', () => {
  const value = Number(heightControl.value);
  term.options.lineHeight = value;
  heightValue.value = value.toFixed(2);
  localStorage.setItem('lineHeight', String(value));
  fit.fit();
  resizePty();
});
document.getElementById('settings-toggle').addEventListener('click', () => {
  settings.hidden = !settings.hidden;
  if (settings.hidden) term.focus();
});
document.addEventListener('keydown', (event) => {
  if (event.ctrlKey && event.shiftKey && event.code === 'KeyL') {
    event.preventDefault();
    settings.hidden = !settings.hidden;
    if (settings.hidden) term.focus();
  } else if (event.code === 'Escape' && !settings.hidden) {
    settings.hidden = true;
    term.focus();
  }
});

function resizePty() {
  invoke('resize_pty', { cols: term.cols, rows: term.rows }).catch(showError);
}
new ResizeObserver(() => {
  fit.fit();
  resizePty();
}).observe(terminalElement);

function showError(error) {
  term.write(`\r\n\x1b[31mWorminal: ${String(error)}\x1b[0m\r\n`);
}

function writeChunk(chunk) {
  const bytes = Uint8Array.from(atob(chunk.data), (character) => character.charCodeAt(0));
  term.write(bytes);
}

// PTY output can arrive while attach() is in flight. Sequence numbers let us
// join the buffered prefix and live stream without dropping or duplicating data.
const pending = new Map();
let nextSequence = null;
function receive(chunk) {
  pending.set(chunk.sequence, chunk);
  drain();
}
function drain() {
  while (nextSequence !== null && pending.has(nextSequence)) {
    writeChunk(pending.get(nextSequence));
    pending.delete(nextSequence++);
  }
}

term.onData((data) => invoke('write_pty', { data }).catch(showError));
async function connect() {
  try {
    await listen('pty-output', ({ payload }) => receive(payload));
    const snapshot = await invoke('attach');
    nextSequence = snapshot.chunks.length ? snapshot.chunks[0].sequence : snapshot.next_sequence;
    snapshot.chunks.forEach(receive);
    for (const [sequence] of pending) if (sequence < nextSequence) pending.delete(sequence);
    drain();
    const theme = await invoke('load_theme');
    term.options.theme = theme;
    document.documentElement.style.background = theme.background || '#181818';
    resizePty();
    term.focus();
  } catch (error) {
    showError(error);
  }
}
connect();
