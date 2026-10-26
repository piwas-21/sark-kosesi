const iconPaths = {
  'arrow-right': '<path d="M4 12h15m-6-6 6 6-6 6"/>',
  'arrow-up-right': '<path d="M6 18 18 6M6 6h12v12"/>',
  bag: '<path d="M5 7h14l1 14H4L5 7Zm3 0V5a4 4 0 0 1 8 0v2"/>',
  globe: '<circle cx="12" cy="12" r="9"/><ellipse cx="12" cy="12" rx="4" ry="9"/><path d="M3 12h18M5 6h14M5 18h14"/>',
  'sun-moon': '<path d="M12 3a9 9 0 1 0 9 9c-6 3-12-3-9-9Z"/><path d="M18 2v4m-2-2h4m1 3v2m-1-1h2"/>',
  room: '<path d="m12 2 9 5v10l-9 5-9-5V7l9-5Zm0 0v10m-9 5 9-5 9 5M3 7l9 5 9-5"/>',
  plus: '<path d="M5 12h14M12 5v14"/>',
  close: '<path d="m6 6 12 12M6 18 18 6"/>',
  pause: '<path d="M9 5v14M15 5v14"/>',
  play: '<path d="m8 4 12 8-12 8V4Z"/>',
  scroll: '<rect x="7" y="2" width="10" height="16" rx="5"/><path d="M12 6v4m-3 10 3 3 3-3"/>',
  delivery: '<path d="M2 5h12v12H2V5Zm12 5h4l4 4v3h-8M3 9h6m-6 3h4"/><circle cx="6" cy="18" r="2"/><circle cx="18" cy="18" r="2"/>',
  layers: '<path d="m12 3 10 5-10 5L2 8l10-5Zm-10 9 10 5 10-5M2 16l10 5 10-5"/>',
  seat: '<path d="M4 12V5h16v7M2 12h20v8H2v-8Zm10-7v7M2 16h20M5 20v2m14-2v2"/>',
  heart: '<path d="M20.5 5.5a5 5 0 0 0-8.5 3 5 5 0 0 0-8.5-3C-.5 10.5 7 17 12 21c5-4 12.5-10.5 8.5-15.5Z"/>',
  thread: '<path d="m12 2 4 6 6 4-6 4-4 6-4-6-6-4 6-4 4-6Z"/><path d="m12 7 5 5-5 5-5-5 5-5Zm-9-4 3 3m12 12 3 3M3 21l3-3M18 6l3-3"/>',
  tea: '<path d="M6 4c0 4 2 5 2 8s-2 5-2 7h12c0-2-2-4-2-7s2-4 2-8H6Zm-2 18h16M8 12h8M10 1v1m4-1v1"/>',
  move: '<path d="M12 2v20M2 12h20M8 6l4-4 4 4M8 18l4 4 4-4M6 8l-4 4 4 4M18 8l4 4-4 4"/>',
  rotate: '<path d="M20 8a9 9 0 1 0 1 7M20 2v6h-6"/>',
  check: '<path d="m4 12 5 5L20 6"/>',
  dashboard: '<rect x="3" y="3" width="7" height="7" rx="1"/><rect x="14" y="3" width="7" height="7" rx="1"/><rect x="3" y="14" width="7" height="7" rx="1"/><rect x="14" y="14" width="7" height="7" rx="1"/>',
  message: '<path d="M3 3h18v14H9l-6 4V3Z"/><path d="M7 8h10M7 12h7"/>',
  users: '<circle cx="9" cy="7" r="4"/><path d="M2 21v-3a7 7 0 0 1 14 0v3M16 3a4 4 0 0 1 0 8m3 3a6 6 0 0 1 3 5v2"/>',
  tag: '<path d="M3 3h9l10 10-9 9L3 12V3Z"/><circle cx="8" cy="8" r="1"/>',
  activity: '<path d="M2 12h4l3-8 6 16 3-8h4"/>'
};
function icon(name) {
  return `<svg class="icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">${iconPaths[name] || iconPaths.thread}</svg>`;
}
function decorateIcons() {
  document.querySelectorAll('[data-icon]').forEach(el => { el.innerHTML = icon(el.dataset.icon); });
}
