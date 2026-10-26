// This file drives the visual prototype; it does not connect commerce services.
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
const stage = document.getElementById('courtyard');
let motionPaused = reducedMotion.matches;
function syncMotion() {
  document.documentElement.classList.toggle('motion-paused', motionPaused);
  const button = document.getElementById('motion-toggle');
  button.setAttribute('aria-pressed', String(motionPaused));
  button.setAttribute('aria-label', motionPaused ? 'Resume decorative motion' : 'Pause decorative motion');
  button.innerHTML = icon(motionPaused ? 'play' : 'pause');
  stage.style.setProperty('--look-x', '0px');
  stage.style.setProperty('--look-y', '0px');
}
document.getElementById('motion-toggle').onclick = () => { motionPaused = !motionPaused; syncMotion(); };
reducedMotion.addEventListener('change', () => { motionPaused = reducedMotion.matches; syncMotion(); });
let frame;
stage.addEventListener('pointermove', event => {
  if (motionPaused || reducedMotion.matches || event.pointerType !== 'mouse') return;
  cancelAnimationFrame(frame);
  frame = requestAnimationFrame(() => {
    const bounds = stage.getBoundingClientRect();
    stage.style.setProperty('--look-x', `${((event.clientX - bounds.left) / bounds.width - .5) * 10}px`);
    stage.style.setProperty('--look-y', `${((event.clientY - bounds.top) / bounds.height - .5) * 6}px`);
  });
});
stage.addEventListener('pointerleave', () => {
  stage.style.setProperty('--look-x', '0px');
  stage.style.setProperty('--look-y', '0px');
});
const textileButton = document.getElementById('textile-hotspot');
const textileDetail = document.getElementById('textile-detail');
function toggleTextile(open) {
  textileDetail.hidden = !open;
  textileButton.setAttribute('aria-expanded', String(open));
  stage.classList.toggle('detail-open', open);
}
textileButton.onclick = () => toggleTextile(textileDetail.hidden);
document.getElementById('close-textile').onclick = () => { toggleTextile(false); textileButton.focus(); };
document.addEventListener('keydown', event => { if (event.key === 'Escape' && !textileDetail.hidden) { toggleTextile(false); textileButton.focus(); } });
document.querySelectorAll('[data-scroll]').forEach(button => button.onclick = () => {
  document.getElementById(button.dataset.scroll).scrollIntoView({ behavior: reducedMotion.matches ? 'instant' : 'smooth' });
});
const adminIcons = { overview: 'dashboard', orders: 'bag', inventory: 'layers', messages: 'message', customers: 'users', discounts: 'tag', reviews: 'heart', health: 'activity' };
document.querySelectorAll('[data-admin]').forEach(button => button.insertAdjacentHTML('afterbegin', icon(adminIcons[button.dataset.admin])));
document.documentElement.classList.toggle('embedded', query.get('embed') === '1');
window.addEventListener('message', event => {
  if (event.origin !== location.origin || event.source !== parent || event.data?.type !== 'preview-view') return;
  view(event.data.view);
});
decorateIcons();
syncMotion();

addEventListener("hashchange", () => { const next = location.hash.slice(1); if (["home", "product", "planner", "admin"].includes(next) && next !== currentView) view(next); });

document.getElementById("language").addEventListener("change", syncMotion);
