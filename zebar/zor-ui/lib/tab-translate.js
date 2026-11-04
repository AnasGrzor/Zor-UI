// Translator tab (end4's sidebarLeft/Translator, which uses Google via translate-shell).
import { $, esc, icon, bridge, config } from './core.js';

export const LANGS = {
  auto: 'Detect language', af: 'Afrikaans', ar: 'Arabic', bn: 'Bengali', bg: 'Bulgarian', ca: 'Catalan', 'zh-CN': 'Chinese (Simplified)',
  'zh-TW': 'Chinese (Traditional)', hr: 'Croatian', cs: 'Czech', da: 'Danish', nl: 'Dutch', en: 'English', et: 'Estonian', fil: 'Filipino',
  fi: 'Finnish', fr: 'French', de: 'German', el: 'Greek', gu: 'Gujarati', he: 'Hebrew', hi: 'Hindi', hu: 'Hungarian', id: 'Indonesian',
  it: 'Italian', ja: 'Japanese', ko: 'Korean', lv: 'Latvian', lt: 'Lithuanian', ms: 'Malay', mr: 'Marathi', no: 'Norwegian', fa: 'Persian',
  pl: 'Polish', pt: 'Portuguese', pa: 'Punjabi', ro: 'Romanian', ru: 'Russian', sr: 'Serbian', sk: 'Slovak', es: 'Spanish', sw: 'Swahili',
  sv: 'Swedish', ta: 'Tamil', te: 'Telugu', th: 'Thai', tr: 'Turkish', uk: 'Ukrainian', ur: 'Urdu', vi: 'Vietnamese',
};

export async function mountTranslate(root) {
  const cfg = await config();
  let src = cfg.translator.source || 'auto', tgt = cfg.translator.target || 'en';
  const opts = (sel, auto) => Object.entries(LANGS).filter(([k]) => auto || k !== 'auto')
    .map(([k, v]) => `<option value="${k}"${k === sel ? ' selected' : ''}>${esc(v)}</option>`).join('');
  root.innerHTML = `
    <div class="tr-langs">
      <select class="field" id="tr-src">${opts(src, true)}</select>
      <button class="icon-btn tonal" id="tr-swap" title="Swap">${icon('swap_horiz')}</button>
      <select class="field" id="tr-tgt">${opts(tgt, false)}</select>
    </div>
    <textarea class="field tr-box" id="tr-in" placeholder="Enter text to translate"></textarea>
    <div class="tr-out-wrap">
      <div class="tr-out scroll" id="tr-out"><span class="muted">Translation</span></div>
      <div class="tr-actions"><span class="muted" id="tr-detected"></span><button class="icon-btn" id="tr-copy" title="Copy">${icon('content_copy')}</button></div>
    </div>`;
  const input = $('#tr-in', root), output = $('#tr-out', root);
  let timer = 0, seq = 0;

  async function translate() {
    const t = input.value.trim();
    if (!t) { output.innerHTML = '<span class="muted">Translation</span>'; $('#tr-detected', root).textContent = ''; return; }
    const my = ++seq;
    output.classList.add('loading');
    const r = await bridge('translate', tgt, src, t);
    if (my !== seq) return;
    output.classList.remove('loading');
    if (r.error) { output.innerHTML = `<div class="err">${esc(r.error)}</div>`; return; }
    output.textContent = r.text;
    $('#tr-detected', root).textContent = src === 'auto' && r.detected ? `Detected: ${LANGS[r.detected] || r.detected}` : '';
  }
  const later = () => { clearTimeout(timer); timer = setTimeout(translate, 650); };
  const persist = () => bridge('config-set', 'translator', JSON.stringify({ source: src, target: tgt }));

  input.addEventListener('input', later);
  $('#tr-src', root).addEventListener('change', e => { src = e.target.value; persist(); translate(); });
  $('#tr-tgt', root).addEventListener('change', e => { tgt = e.target.value; persist(); translate(); });
  $('#tr-swap', root).addEventListener('click', () => {
    if (src === 'auto') src = $('#tr-detected', root).textContent ? Object.keys(LANGS).find(k => $('#tr-detected', root).textContent.endsWith(LANGS[k])) || 'en' : 'en';
    [src, tgt] = [tgt, src];
    $('#tr-src', root).value = src; $('#tr-tgt', root).value = tgt;
    if (output.textContent && !output.querySelector('.muted, .err')) input.value = output.textContent;
    persist(); translate();
  });
  $('#tr-copy', root).addEventListener('click', () => navigator.clipboard?.writeText(output.textContent));
  return { focus: () => input.focus() };
}
