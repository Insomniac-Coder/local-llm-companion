/* The Next Word Is Ours — slide deck logic. Plain script, no dependencies, works from file://. */
(() => {
  'use strict';

  /* ------------------------------------------------------------------ settings */
  const INR_PER_USD = 95.96;   // rupees per US dollar; every price on the page is derived from this

  /* ------------------------------------------------------------------ helpers */
  const $ = (s, r = document) => r.querySelector(s);
  const $$ = (s, r = document) => [...r.querySelectorAll(s)];
  const html = document.documentElement;
  const reduced = matchMedia('(prefers-reduced-motion: reduce)').matches;
  const clamp = (v, a = 0, b = 1) => Math.min(b, Math.max(a, v));
  const sleep = ms => new Promise(r => setTimeout(r, ms));
  const fmt = n => Math.round(n).toLocaleString('en-US');
  const softmax = (logits, t = 1) => {
    const m = Math.max(...logits);
    const e = logits.map(x => Math.exp((x - m) / t));
    const s = e.reduce((a, b) => a + b, 0);
    return e.map(x => x / s);
  };
  const el = (tag, cls, text) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  };
  const HL = ['var(--hl1)', 'var(--hl2)', 'var(--hl3)', 'var(--hl4)', 'var(--hl5)'];

  /* ------------------------------------------------------------------ money: rupees first, dollars in brackets */
  const num1 = x => (x >= 100 ? Math.round(x).toLocaleString('en-IN') : x >= 10 ? String(Math.round(x)) : String(Math.round(x * 10) / 10));
  const inrParts = r => (r >= 1e7 ? [num1(r / 1e7), ' crore'] : r >= 1e5 ? [num1(r / 1e5), ' lakh'] : [Math.round(r).toLocaleString('en-IN'), '']);
  const usdParts = d => (d >= 1e6 ? [num1(d / 1e6), 'M'] : d >= 1e3 ? [num1(d / 1e3), 'K'] : [String(Math.round(d)), '']);
  const inr = r => { const [n, u] = inrParts(r); return `₹${n}${u}`; };
  const usd = r => { const [n, u] = usdParts(r / INR_PER_USD); return `$${n}${u}`; };
  const both = r => `${inr(r)} (${usd(r)})`;
  $$('[data-rate]').forEach(n => { n.textContent = `₹${INR_PER_USD}`; });

  /* ------------------------------------------------------------------ tokenizer (approximate) */
  // Illustration only: real tokenizers learn their pieces from data. This keeps common words whole,
  // splits long rare words at familiar endings, and gives punctuation and digit groups their own tokens.
  const WHOLE = new Set(('about after again against because before being between business company could customer customers ' +
    'document documents every first going important information language little machine models number other people really ' +
    'should something their there these thing think those through today under using where which while would years answer ' +
    'answers private security server servers contract contracts policy product revenue support tickets network ourselves ' +
    'quarterly knowledge inside council proposal harness training trained')
    .split(' '));
  const SUFFIXES = ['ization', 'isation', 'ations', 'ation', 'ments', 'ment', 'ness', 'ities', 'ity', 'ings', 'ing', 'ible', 'able',
    'less', 'ful', 'ous', 'ive', 'ise', 'ize', 'ers', 'est', 'ed', 'er', 'ly', 'al', 'es'];
  const chunk = s => {
    if (s.length <= 6 || WHOLE.has(s.toLowerCase())) return [s];
    const out = [];
    for (let i = 0; i < s.length;) {
      const left = s.length - i;
      const len = left <= 6 ? left : 4 + (out.length % 2);
      out.push(s.slice(i, i + len));
      i += len;
    }
    return out;
  };
  const splitWord = w => {
    const lw = w.toLowerCase();
    if (w.length <= 6 || WHOLE.has(lw)) return [w];
    for (const suf of SUFFIXES) {
      if (lw.endsWith(suf) && w.length - suf.length >= 3) return [...chunk(w.slice(0, -suf.length)), w.slice(-suf.length)];
    }
    return chunk(w);
  };
  const PIECE = /\s*[A-Za-zÀ-ɏ]+|['’][a-z]+|\s*\d+|\s*[^\s\w]|\s+/gu;
  function tokenize(text) {
    const out = [];
    for (const [s] of text.matchAll(PIECE)) {
      let lead = s.match(/^\s*/)[0];
      const core = s.slice(lead.length);
      if (lead.length > 1 || lead.includes('\n')) { out.push(lead); lead = ''; }
      if (!core) { if (lead) out.push(lead); continue; }
      let parts;
      if (/^\d+$/.test(core)) parts = core.match(/\d{1,3}/g);
      else if (/^[A-Za-zÀ-ɏ]+$/.test(core)) parts = splitWord(core);
      else parts = [core];
      parts[0] = lead + parts[0];
      out.push(...parts);
    }
    return out;
  }

  /* headings: split into tokens, each with its own highlighter colour */
  $$('[data-gen]').forEach(h => {
    let i = 0;
    const walker = document.createTreeWalker(h, NodeFilter.SHOW_TEXT);
    const nodes = [];
    while (walker.nextNode()) nodes.push(walker.currentNode);
    for (const node of nodes) {
      const frag = document.createDocumentFragment();
      for (const t of tokenize(node.nodeValue)) {
        // the space before a word stays outside its span, so a span never breaks across two lines
        const lead = t.match(/^\s*/)[0];
        if (lead) frag.append(lead);
        if (lead === t) continue;
        const s = el('span', 'tk', t.slice(lead.length));
        s.style.setProperty('--i', i);
        s.style.setProperty('--hl', HL[i++ % HL.length]);
        frag.append(s);
      }
      node.replaceWith(frag);
    }
  });
  if (!reduced) html.classList.add('js-motion');
  const generate = slide => $$('[data-gen]', slide).forEach(h => { h.classList.remove('gen'); void h.offsetWidth; h.classList.add('gen'); });

  /* ------------------------------------------------------------------ deck: one slide per screen */
  const slides = $$('.slide');
  const track = $('#track');
  const bar = $('#bar');
  const drawer = $('#drawer');
  const engBtn = $('#engBtn');
  const hooks = {};            // slide id -> function run when the slide comes into view
  const visible = new Set();   // flow mode: slides currently on screen
  let cur = 0;
  let S = 1;                   // current scale of the 1600 × 860 canvas
  let animating = false;
  const isDeck = () => html.classList.contains('deck');
  const isActive = id => (isDeck() ? slides[cur].id === id : visible.has(id));

  function fit() {
    const want = innerWidth >= 900 && innerHeight >= 520 ? 'deck' : 'flow';
    const changed = !html.classList.contains(want);
    if (changed) { html.classList.remove('deck', 'flow'); html.classList.add(want); }
    if (isDeck()) {
      const vh = innerHeight - bar.offsetHeight;
      S = Math.min(innerWidth / 1600, vh / 860);
      html.style.setProperty('--vh', vh + 'px');
      html.style.setProperty('--s', S);
    } else {
      S = 1;
    }
    return changed;
  }

  function enter(i) {
    const s = slides[i];
    html.dataset.tone = s.classList.contains('dark') ? 'dark' : 'light';
    generate(s);
    $$('[data-count]', s).forEach(countUp);
    hooks[s.id]?.();
  }

  function go(i, instant = false) {
    i = clamp(i, 0, slides.length - 1);
    if (!isDeck()) { slides[i].scrollIntoView({ behavior: reduced || instant ? 'auto' : 'smooth' }); return; }
    const same = i === cur && slides[i].classList.contains('on');
    cur = i;
    if (instant || reduced) track.style.transition = 'none';
    html.style.setProperty('--i', i);
    if (instant || reduced) { void track.offsetHeight; track.style.transition = ''; }
    slides.forEach((s, k) => s.classList.toggle('on', k === i));
    animating = !instant && !reduced;
    setTimeout(() => { animating = false; }, 820);
    if (!same) enter(i);
    updateBar();
    closeDrawer();
    try { history.replaceState(null, '', '#' + slides[i].id); } catch { /* sandboxed: fine */ }
  }

  function updateBar() {
    const n = slides.length;
    $('#barLabel').textContent = slides[cur].dataset.label || '';
    $('#barCount').textContent = `${String(cur + 1).padStart(2, '0')} / ${n}`;
    $('#barFill').style.setProperty('--p', cur / (n - 1));
    $('#prevBtn').disabled = cur === 0;
    $('#nextBtn').disabled = cur === n - 1;
    engBtn.hidden = !$('.eng-note', slides[cur]);
  }

  function openDrawer() {
    const note = $('.eng-note', slides[cur]);
    if (!note) return;
    $('#drawerBody').replaceChildren(...$$('p', note).map(p => p.cloneNode(true)));
    drawer.hidden = false;
    engBtn.setAttribute('aria-expanded', 'true');
  }
  function closeDrawer() {
    drawer.hidden = true;
    engBtn.setAttribute('aria-expanded', 'false');
  }
  engBtn.addEventListener('click', () => (drawer.hidden ? openDrawer() : closeDrawer()));
  $('#drawerClose').addEventListener('click', closeDrawer);
  $('#prevBtn').addEventListener('click', () => go(cur - 1));
  $('#nextBtn').addEventListener('click', () => go(cur + 1));

  // Anything that can scroll on its own (the note drawer, a long text box) keeps the wheel.
  function scrollsItself(target, dy) {
    for (let n = target; n && n !== document.body; n = n.parentElement) {
      if (n.classList?.contains('slide')) return false;
      const oy = getComputedStyle(n).overflowY;
      if ((oy === 'auto' || oy === 'scroll' || n.tagName === 'TEXTAREA') && n.scrollHeight > n.clientHeight + 1) {
        if (dy > 0 && n.scrollTop + n.clientHeight < n.scrollHeight - 1) return true;
        if (dy < 0 && n.scrollTop > 0) return true;
      }
    }
    return false;
  }

  // One wheel gesture = one slide. A trackpad keeps sending events after a flick (inertia), so after
  // moving, the rest of that stream is ignored until there is a short pause.
  let wheelAcc = 0, lastWheel = 0, lastNav = -1e9;
  addEventListener('wheel', e => {
    if (!isDeck() || e.ctrlKey) return;
    const dy = Math.abs(e.deltaY) >= Math.abs(e.deltaX) ? e.deltaY : e.deltaX;
    if (scrollsItself(e.target, dy)) return;
    e.preventDefault();
    const now = performance.now();
    const gap = now - lastWheel;
    lastWheel = now;
    if (gap > 180) wheelAcc = 0;
    if (animating || (gap < 180 && now - lastNav < 2500)) return;
    wheelAcc += dy;
    if (Math.abs(wheelAcc) < 40) return;
    const dir = Math.sign(wheelAcc);
    wheelAcc = 0;
    lastNav = now;
    go(cur + dir);
  }, { passive: false });

  addEventListener('keydown', e => {
    if (e.key === 'Escape') { closeDrawer(); return; }
    if (!isDeck() || e.altKey || e.ctrlKey || e.metaKey) return;
    const t = e.target;
    if (t.closest?.('textarea, select, [contenteditable], input:not([type="range"]):not([type="checkbox"])')) return;
    const onRange = t.matches?.('input[type="range"]');
    const onButton = t.closest?.('button, a, label');
    let d = 0;
    switch (e.key) {
      case 'ArrowRight': case 'ArrowDown': if (!onRange) d = 1; break;
      case 'ArrowLeft': case 'ArrowUp': if (!onRange) d = -1; break;
      case 'PageDown': d = 1; break;
      case 'PageUp': d = -1; break;
      case ' ': if (!onButton && !onRange) d = e.shiftKey ? -1 : 1; break;
      case 'Home': go(0); e.preventDefault(); return;
      case 'End': go(slides.length - 1); e.preventDefault(); return;
      default: return;
    }
    if (!d) return;
    e.preventDefault();
    if (!animating) go(cur + d);
  });

  let tx = 0, ty = 0;
  const viewport = $('#viewport');
  viewport.addEventListener('touchstart', e => { const p = e.touches[0]; tx = p.clientX; ty = p.clientY; }, { passive: true });
  viewport.addEventListener('touchend', e => {
    if (!isDeck() || e.target.closest('input, textarea, select')) return;
    const p = e.changedTouches[0];
    const dx = p.clientX - tx, dy = p.clientY - ty;
    if (Math.max(Math.abs(dx), Math.abs(dy)) < 50) return;
    go(cur + (Math.abs(dy) > Math.abs(dx) ? -Math.sign(dy) : -Math.sign(dx)));
  }, { passive: true });

  // links to other slides ("#what") move the deck instead of the page
  document.addEventListener('click', e => {
    const a = e.target.closest('a[href^="#"]');
    if (!a) return;
    const target = document.getElementById(a.getAttribute('href').slice(1));
    const slide = target?.closest('.slide');
    if (!slide) return;
    e.preventDefault();
    go(slides.indexOf(slide));
  });

  // flow mode: slides fire their hooks as they scroll into view
  if ('IntersectionObserver' in window) {
    const fio = new IntersectionObserver(es => {
      for (const e of es) {
        const id = e.target.id;
        if (e.isIntersecting) {
          if (isDeck() || visible.has(id)) continue;
          visible.add(id);
          cur = slides.indexOf(e.target);
          enter(cur);
        } else {
          visible.delete(id);
        }
      }
    }, { threshold: 0.35 });
    slides.forEach(s => fio.observe(s));
  }

  /* shared probability bars */
  function setBars(box, items, chosen = -1) {
    if (box.children.length !== items.length) {
      box.replaceChildren(...items.map(() => {
        const r = el('div', 'bar-row');
        r.append(el('span', 'bar-label'), el('span', 'bar-track'), el('span', 'bar-val'));
        r.children[1].append(el('span', 'bar-fill'));
        return r;
      }));
    }
    items.forEach((it, i) => {
      const r = box.children[i];
      r.children[0].textContent = it.label;
      r.children[2].textContent = (it.p * 100 < 1 && it.p > 0 ? '<1' : Math.round(it.p * 100)) + '%';
      r.children[1].firstChild.style.setProperty('--w', it.p);
      r.classList.toggle('chosen', i === chosen);
    });
  }
  const paintRange = r => r.style.setProperty('--fill', ((r.value - r.min) / (r.max - r.min) * 100) + '%');
  $$('.range').forEach(r => { paintRange(r); r.addEventListener('input', () => paintRange(r)); });

  function countUp(n) {
    const end = +n.dataset.count;
    if (!end || reduced) return;
    const t0 = performance.now();
    const tick = now => {
      const k = clamp((now - t0 - 350) / 1100);
      n.textContent = fmt(end * (1 - Math.pow(1 - k, 3)));
      if (k < 1) requestAnimationFrame(tick);
    };
    n.textContent = '0';
    requestAnimationFrame(tick);
  }

  /* ------------------------------------------------------------------ 1: a model writing */
  const SEQS = [
    { prompt: "The safest place for our company's knowledge is", steps: [
      [[' inside', .41], [' our', .22], [' a', .14], [' the', .12], [' behind', .06]],
      [[' our', .78], [' the', .09], [' a', .05], [' its', .04], [' your', .02]],
      [[' own', .52], [' network', .21], [' walls', .11], [' building', .08], [' servers', .05]],
      [[' servers', .44], [' network', .27], [' walls', .12], [' building', .09], [' hardware', .05]],
      [['.', .83], [',', .08], [' and', .05], [' where', .02], ['!', .01]],
    ] },
    { prompt: 'A language model writes its answer', steps: [
      [[' one', .61], [' by', .12], [' in', .10], [' word', .07], [' using', .05]],
      [[' word', .57], [' token', .31], [' piece', .05], [' step', .04], [' line', .02]],
      [[' at', .93], [' after', .04], [' by', .02], [' per', .005], [' each', .005]],
      [[' a', .97], [' the', .02], [' each', .005], [' one', .003], [' any', .002]],
      [[' time', .98], [' go', .01], [' moment', .005], [' step', .003], [' turn', .002]],
      [['.', .88], [',', .07], [' and', .03], ['!', .01], [' from', .01]],
    ] },
    { prompt: 'With our own AI, sensitive documents never leave', steps: [
      [[' the', .48], [' our', .35], [' your', .07], [' their', .05], [' this', .03]],
      [[' building', .46], [' company', .26], [' network', .17], [' office', .07], [' country', .03]],
      [['.', .9], [',', .05], [' again', .02], ['!', .02], [' and', .01]],
    ] },
  ];
  const genPrompt = $('.gen-prompt');
  const genOut = $('.gen-out');
  const genBars = $('#genBars');
  const genPause = $('#genPause');
  const label = t => (t.trim() ? `"${t.trim()}"` : t);
  const toItems = step => step.map(([t, p]) => ({ label: label(t), p }));
  let genPaused = false;
  async function running() { while (genPaused || !isActive('top') || document.hidden) await sleep(150); }
  async function runGen() {
    for (let si = 0; ; si++) {
      const seq = SEQS[si % SEQS.length];
      genPrompt.textContent = seq.prompt;
      genOut.replaceChildren();
      for (const step of seq.steps) {
        await running();
        setBars(genBars, step.map(([t]) => ({ label: label(t), p: 0 })));
        await sleep(30);
        setBars(genBars, toItems(step));
        await sleep(950);
        await running();
        setBars(genBars, toItems(step), 0);
        await sleep(520);
        genOut.append(el('span', 'new', step[0][0]));
        await sleep(480);
      }
      await sleep(2800);
    }
  }
  if (reduced) {
    genOut.textContent = SEQS[0].steps.map(s => s[0][0]).join('');
    setBars(genBars, toItems(SEQS[0].steps[3]), 0);
    genPause.hidden = true;
  } else {
    genPause.addEventListener('click', () => {
      genPaused = !genPaused;
      genPause.textContent = genPaused ? 'Play' : 'Pause';
      genPause.setAttribute('aria-pressed', String(genPaused));
    });
    setBars(genBars, toItems(SEQS[0].steps[0]));
    runGen();
  }

  /* ------------------------------------------------------------------ 4: six steps of inference */
  const stepBtns = $$('#steps .step');
  const framesV = $$('.frame-v');
  let stepNow = 1, autoplay = true, autoTimer = 0;
  function showStep(n) {
    stepNow = n;
    stepBtns.forEach(b => b.classList.toggle('is-on', +b.dataset.step === n));
    framesV.forEach(f => f.classList.toggle('is-on', +f.dataset.frame === n));
    if (n === 3) requestAnimationFrame(drawAttention);
    if (n === 6) runLoop();
  }
  function scheduleAuto() {
    clearTimeout(autoTimer);
    if (!autoplay || reduced) return;
    autoTimer = setTimeout(() => {
      if (!isActive('work') || !autoplay) return;
      showStep(stepNow % 6 + 1);
      scheduleAuto();
    }, stepNow === 3 || stepNow === 6 ? 7000 : 5000);
  }
  stepBtns.forEach(b => b.addEventListener('click', () => { autoplay = false; clearTimeout(autoTimer); showStep(+b.dataset.step); }));
  hooks.work = () => { if (autoplay) { showStep(1); scheduleAuto(); } else if (stepNow === 3) drawAttention(); };

  // step 2: embedding strips (a fixed pseudo-random pattern; "agreement" deliberately resembles "contract")
  const seeded = seed => () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
  const EMB = [['Summ', 11], ['arise', 23], [' this', 5], [' contract', 42], [' agreement *', 42], [' for', 7], [' me', 19]];
  $('#embRows').replaceChildren(...EMB.map(([tok, seed], k) => {
    const row = el('div', 'emb-row');
    row.append(el('span', null, tok.trim()));
    const cells = el('div', 'emb-cells');
    const rnd = seeded(seed * 7919), jitter = seeded(k * 131 + 3);
    for (let i = 0; i < 24; i++) {
      const c = el('i');
      c.style.opacity = clamp(rnd() + (k === 4 ? (jitter() - 0.5) * 0.18 : 0), 0.08, 1).toFixed(2);
      cells.append(c);
    }
    row.append(cells);
    return row;
  }));

  // step 3: attention
  const SENTS = [
    { words: ['I', 'deposited', 'cash', 'at', 'the', 'bank'], w: [0.04, 0.34, 0.4, 0.06, 0.06],
      meaning: 'The model reads “bank” as <b>a place for money</b>, mostly because of “deposited” and “cash”.' },
    { words: ['We', 'had', 'a', 'picnic', 'by', 'the', 'river', 'bank'], w: [0.03, 0.03, 0.02, 0.2, 0.08, 0.04, 0.6],
      meaning: 'The model reads “bank” as <b>the side of a river</b>, mostly because of “river” and “picnic”.' },
  ];
  let sentIdx = 0;
  const attnWords = $('#attnWords'), attnSvg = $('#attnSvg'), attnStage = $('#attnStage');
  function renderAttention() {
    const s = SENTS[sentIdx];
    attnWords.replaceChildren(...s.words.map((w, i) => {
      const n = el('span', 'aw' + (i === s.words.length - 1 ? ' focus' : ''), w);
      if (i < s.w.length) n.style.setProperty('--a', Math.min(1, s.w[i] * 1.6).toFixed(2));
      return n;
    }));
    $('#attnMeaning').innerHTML = s.meaning;
    drawAttention();
  }
  function drawAttention() {
    const s = SENTS[sentIdx];
    const box = attnStage.getBoundingClientRect();
    if (!box.width) return;
    const pos = [...attnWords.children].map(c => {
      const r = c.getBoundingClientRect();
      return { x: (r.left - box.left + r.width / 2) / S, y: (r.top - box.top) / S };
    });
    const f = pos[pos.length - 1];
    attnSvg.setAttribute('viewBox', `0 0 ${box.width / S} ${box.height / S}`);
    attnSvg.innerHTML = s.w.map((w, i) => {
      const p = pos[i];
      const lift = Math.min(86, 24 + Math.abs(f.x - p.x) * 0.26) + Math.max(0, f.y - p.y);
      return `<path class="attn-line" d="M${f.x} ${f.y} C ${f.x} ${f.y - lift}, ${p.x} ${p.y - lift}, ${p.x} ${p.y}" stroke-width="${(1 + w * 10).toFixed(1)}" opacity="${(0.15 + w * 0.85).toFixed(2)}"/>`;
    }).join('');
  }
  $$('#attnSeg .seg-btn').forEach(b => b.addEventListener('click', () => {
    autoplay = false; clearTimeout(autoTimer);
    sentIdx = +b.dataset.sent;
    $$('#attnSeg .seg-btn').forEach(x => x.classList.toggle('is-on', x === b));
    renderAttention();
  }));
  renderAttention();

  // step 6: the loop
  const LOOP = ['This', ' contract', ' sets', ' out', ' a', ' two', '-year', ' supply', ' agreement', ' between', ' us', ' and', ' Acme', ' Ltd', '.'];
  const loopText = $('#loopText');
  let loopRun = 0;
  async function runLoop() {
    const me = ++loopRun;
    if (reduced) { loopText.textContent = LOOP.join(''); return; }
    while (stepNow === 6 && me === loopRun && isActive('work')) {
      loopText.replaceChildren();
      for (const t of LOOP) {
        if (stepNow !== 6 || me !== loopRun) return;
        loopText.append(el('span', 'new', t));
        await sleep(170);
      }
      await sleep(1800);
    }
  }
  loopText.textContent = LOOP.join('');

  /* BUILD:controls START */
  /* ------------------------------------------------------------------ 5: temperature and top-K */
  /* BUILD:controls END */

  /* BUILD:memory START */
  /* ------------------------------------------------------------------ 6: memory: without and with */
  /* BUILD:memory END */

  /* ------------------------------------------------------------------ 8: glossary */
  const CATS = { basics: 'Basics', build: 'Building with AI', run: 'Running a model', risk: 'Risks and safety' };
  const TERMS = [
    ['Token', 'basics', 1, 'The unit a model reads and writes: a common word, or a piece of a rarer one. Usage limits and prices are counted in tokens.', 'Syllables, for a machine.', 'Tokenizers are learned from data, usually by byte-pair encoding. Vocabularies run from about 32,000 to over 200,000 tokens. English averages about 0.75 words per token; many other languages, and code, need more tokens for the same meaning.'],
    ['Parameters', 'basics', 1, 'The billions of numbers inside a model that were adjusted during training. Together they hold everything it learned.', 'The strength of the connections in a brain.', 'Also called weights. An 8B model has 8 billion of them. Memory needed ≈ parameters × bytes per parameter: 2 bytes at 16-bit, about 0.5 at 4-bit.'],
    ['Context window', 'basics', 1, 'How much text the model can consider at once: the instructions, the conversation so far and any documents supplied, measured in tokens.', 'A desk. Only what fits on the desk gets used.', 'Ranges from about 8,000 to over 1,000,000 tokens. Longer contexts need more memory (the KV cache), and models often miss facts buried in the middle of very long inputs, which is one reason RAG sends only the relevant passages.'],
    ['Prompt and system prompt', 'basics', 0, 'The prompt is what a person sends. The system prompt is standing instructions set by whoever runs the tool: role, rules, tone and format.', 'The briefing a new employee gets on day one, then the day’s requests.', 'In a harness, the system prompt also describes the available tools and how to call them. It is one of the most important levers we control; with a subscription service most of it belongs to the vendor.'],
    ['Inference', 'basics', 0, 'Using a trained model to produce an answer. It happens every time anyone asks a question.', 'Training is school. Inference is doing the job.', 'For a single user the cost is dominated by GPU memory and memory bandwidth rather than raw compute. Batching many users together is what makes a shared server efficient.'],
    ['Temperature', 'basics', 0, 'A setting that controls how adventurous the model is when it picks each next word. Low is predictable, high is creative.', 'A careful accountant versus a brainstorming session.', 'Probabilities are computed as softmax(scores ÷ temperature). Related settings, top-p and top-k, cut unlikely tokens out entirely.'],
    ['Knowledge cut-off', 'basics', 0, 'The date the training data ends. The model knows nothing after it unless the harness gives it documents or search results.', 'A well-read colleague who has been off-grid since that date.', 'Cut-offs are usually several months to a year before a model is released, and models are often unsure of their own cut-off.'],
    ['Multimodal', 'basics', 0, 'A model that handles more than text: images, scanned documents, charts, audio, sometimes video.', 'A colleague who reads the attachment, not only the email.', 'Usually a vision encoder turns an image into features the language model can read. Several open-weight families (Gemma 3, Llama 4, Mistral Small 3.1) are multimodal.'],
    ['Reasoning model', 'basics', 0, 'A model trained to work through a problem step by step before answering. Slower, but much better at maths, code and planning.', 'Showing your working in an exam.', 'Examples: OpenAI’s o-series and its open-weight gpt-oss, Mistral’s Magistral. The thinking costs extra tokens, so answers take longer and use more compute.'],
    ['RAG', 'build', 1, 'Retrieval-augmented generation. Before answering, the system searches our documents for the relevant passages and gives them to the model with the question, so it answers from them and can cite them.', 'An open-book exam instead of answering from memory.', 'Split documents into passages, embed them, store them in an index; at question time search (vector plus keyword), rerank, and put the best passages into the prompt. Updating knowledge means updating the index: no retraining.'],
    ['Embeddings', 'build', 0, 'A way to turn a piece of text into a list of numbers that captures its meaning, so software can find passages that mean the same thing even when the words differ.', 'Map coordinates for meaning: similar ideas sit close together.', 'Made by a separate, small embedding model, typically 384 to 4,096 numbers per passage. Closeness is measured by the angle between two lists (cosine similarity).'],
    ['Vector database', 'build', 0, 'A database that stores embeddings and quickly finds the ones closest to a question. It is the search engine behind RAG.', 'A library catalogue organised by topic instead of title.', 'Options include PostgreSQL with pgvector, Qdrant, Milvus and OpenSearch. Store each passage’s access rights next to it, so search only returns what the asker may see.'],
    ['Fine-tuning', 'build', 1, 'Further training an existing model on our own examples, to change its behaviour, tone or output format.', 'On-the-job training for a graduate who already has a degree.', 'Good for style, format and narrow tasks such as sorting tickets. Poor at reliably adding facts, and whatever it learns is frozen on the day training ends. Needs hundreds to thousands of high-quality examples.'],
    ['LoRA', 'build', 0, 'A cheap way to fine-tune. Instead of changing every parameter, a small add-on is trained: hours on one GPU instead of weeks on many.', 'Clip-on lenses instead of new glasses.', 'Low-Rank Adaptation trains small extra matrices, typically well under 1% of the model’s size. QLoRA does the same on a 4-bit model to save memory. Different adapters can be swapped in per task on one base model.'],
    ['Harness', 'build', 1, 'All the software around a model that turns it into a useful tool: instructions, memory, access to documents and tools, permissions and logging.', 'The model is the engine; the harness is the rest of the car.', 'Examples: the ChatGPT app, Claude Code, and our own PC Companion. The harness decides what goes into the context window at every step, so it drives quality as much as the model does.'],
    ['Agent', 'build', 0, 'A model inside a harness that works through a task in steps: search, read, calculate, write, check its own work, and repeat until done.', 'An assistant you give a goal, not a single instruction.', 'A loop of model call → tool call → result → model call. Agents need limits: approval for risky actions, detection of loops, and a clear definition of done.'],
    ['Tool calling', 'build', 0, 'The model’s ability to ask the harness to run something, such as a search, a database query or a calculation, and then use the result.', 'Asking a colleague to check the system while you draft the reply.', 'Also called function calling. The model writes a structured request that matches a tool’s description; the harness checks it and runs it. The model itself never executes anything.'],
    ['MCP', 'build', 0, 'Model Context Protocol: an open standard for connecting AI tools to data sources and apps, so a connector is built once and works with many models.', 'USB for AI: one plug shape for many devices.', 'Introduced by Anthropic in November 2024 and since adopted widely, including by OpenAI and Google. An MCP server exposes tools and data; the harness is the client.'],
    ['Open-weight model', 'run', 1, 'A model whose trained parameters are published for anyone to download and run on their own hardware.', 'Getting the recipe, not just the meal.', 'Examples: Llama (Meta), Gemma (Google), Mistral, gpt-oss (OpenAI), Phi (Microsoft). Not the same as open source: training data and code are usually not released, and licences range from fully permissive (Apache 2.0) to custom terms with conditions.'],
    ['Quantization', 'run', 0, 'Storing each parameter with fewer bits, for example 4 instead of 16, so the model needs about a quarter of the memory for a small loss in quality.', 'A high-quality MP3 of a studio recording.', 'Common formats: GGUF (llama.cpp), AWQ and GPTQ (GPU servers), FP8. 4-bit keeps most of the quality for everyday work; 2 or 3 bits start to hurt noticeably.'],
    ['GPU and VRAM', 'run', 0, 'The graphics processor does the maths; its own memory (VRAM) has to hold the model. VRAM size decides which models a machine can run.', 'The size of the workbench decides how big a job you can take on.', 'Consumer cards have 8 to 32 GB, data-centre cards 48 GB and up. A model that does not fit spills into system memory at a large speed cost. Serving software: llama.cpp for single machines, vLLM or SGLang for many users.'],
    ['Tokens per second', 'run', 0, 'How fast a model writes. People read at about 5 tokens per second; a good private setup writes 30 to 100 or more.', 'Words per minute, for a machine.', 'Two numbers matter: prompt processing (reading, often thousands of tokens per second) and generation (writing). Generation speed is limited mainly by memory bandwidth.'],
    ['Mixture of experts', 'run', 0, 'A model split into many specialist sub-networks, with only a few switched on for each token. Big-model knowledge at small-model speed.', 'A hospital full of specialists, where each patient sees only two or three.', 'Llama 4 Maverick: 400B parameters, 17B active per token. gpt-oss-120b: 117B total, about 5B active. All experts still have to fit in memory.'],
    ['Distillation', 'run', 0, 'Training a smaller model to imitate a bigger one. Many of the best small models are made this way.', 'An apprentice learning from a master.', 'The student learns from the teacher’s outputs. Google’s Gemma 3 models, for example, were trained by distillation from a larger teacher model.'],
    ['Top-K', 'basics', 0, 'A setting that lets the model choose the next word only from its K most likely candidates, cutting every unlikely word out before it picks.', 'A shortlist: only the top five candidates get an interview.', 'Applied after temperature: keep the K most probable tokens, rescale them to add up to 100%, then pick. Top-p keeps the smallest set whose chances add up to p instead. In RAG, “top-K” also means how many passages the retriever returns.'],
    ['Memory', 'basics', 1, 'How a chat system keeps track of a conversation. The model itself remembers nothing between calls; the software sends the earlier messages back with every new question.', 'A colleague with no memory, handed the meeting notes before every reply.', 'Short-term memory is the message thread, saved under a thread ID (in LangGraph, by a checkpointer) and trimmed or summarised as it grows. Long-term memory is a separate store of facts the harness looks up. Every resent message costs tokens.'],
    ['Chunking', 'build', 0, 'Cutting long documents into short passages before they are stored for search, so the system can find and quote exactly the part that answers a question.', 'Cutting a book into index cards.', 'Typical chunks are a few hundred tokens, overlapping by 10–20% so no sentence is cut in half. Too small loses context; too large dilutes the match. Splitting on headings and paragraphs beats fixed lengths.'],
    ['ChromaDB', 'build', 0, 'An open-source vector database. It stores each chunk with its embedding and details such as source and page, and finds the chunks closest in meaning to a question.', 'A filing cabinet sorted by meaning instead of by name.', 'Runs inside the application or as its own server on our machines. Collections hold ids, embeddings, text and metadata; a query returns the nearest neighbours, optionally filtered by metadata such as department or access level.'],
    ['LangChain', 'build', 0, 'An open-source toolkit for building applications on language models: model connectors, prompts, retrievers and tools that snap together.', 'A box of standard parts for building with AI.', 'Gives one interface over many model servers, including private ones with an OpenAI-compatible API, plus document loaders, text splitters, vector-store retrievers (ChromaDB among them) and tool definitions.'],
    ['LangGraph', 'build', 1, 'A framework from the LangChain team for building agents as graphs: steps are nodes, arrows decide what runs next, and loops, branches and pauses are allowed.', 'A flowchart that can loop back, take a decision and wait for sign-off.', 'A StateGraph holds one shared, typed state; nodes update it and conditional edges route on it. A checkpointer saves the state after every step, which gives memory, resumable runs, replay and human-in-the-loop pauses.'],
    ['Supervisor agent', 'build', 0, 'An agent whose job is to read each request, hand it to the right specialist agent, and decide when the work is done.', 'A team lead who assigns the ticket.', 'Built as a graph node that returns the name of the next agent, or “finish”. In our proof of concept it routes between a RAG agent, an infrastructure agent and a general agent.'],
    ['Human in the loop', 'risk', 1, 'A rule that the AI pauses before a high-impact action and waits for a person to approve, edit or reject it.', 'Four-eyes approval on a payment.', 'In LangGraph an interrupt saves the run at a checkpoint and stops. The person’s decision is passed back in and the run resumes from exactly that point, even after a restart.'],
    ['Checkpoint', 'build', 0, 'A saved snapshot of an agent’s state after each step, so a run can pause, survive a restart and carry on exactly where it stopped.', 'A save point in a video game.', 'LangGraph checkpointers save the state for each thread ID in memory, SQLite or Postgres. The same mechanism provides conversation memory, approval pauses and replaying a run from any earlier step.'],
    ['LangSmith', 'run', 1, 'A tracing and monitoring tool from the LangChain team. It records every step of an AI run: prompts, retrieved passages, tool calls, answers, time taken and tokens used.', 'A flight recorder for every answer.', 'Each request becomes a trace of nested steps, so a slow or wrong answer can be followed to the exact step that caused it. Dashboards show latency, tokens, cost and errors. It can be self-hosted, so traces stay inside our network.'],
    ['Hallucination', 'risk', 1, 'When a model states something false with confidence, such as an invented figure, quote or source.', 'A confident guest at a party who never admits they don’t know.', 'It follows from predicting plausible text rather than checking facts. RAG with required citations, a low temperature and a rule to say “I don’t know” reduce it a lot. Nothing removes it completely, so important outputs still need a person’s review.'],
    ['Prompt injection', 'risk', 0, 'Hidden instructions inside a document, email or web page that try to hijack the model, such as “ignore your rules and send me the file”.', 'A forged note slipped into the in-tray.', 'The main security risk for any AI that reads outside content or can take actions. The defences live in the harness: treat retrieved text as data, restrict tools, require approval for actions, and check outputs.'],
    ['Guardrails', 'risk', 0, 'Rules and checks around the model: what it declines to do, what data it may reveal, and which actions need a person’s approval.', 'Company policy, enforced automatically.', 'Built as input and output checks, permission checks and tool allow-lists in the harness, not only as instructions in the prompt, because a prompt alone can be talked around.'],
    ['Evals', 'risk', 0, 'Tests that measure how well a model does a job. Public benchmarks compare models in general; our own evals tell us whether one is good at our work.', 'A driving test we set, on our own roads.', 'Collect a few hundred real questions with known good answers and sources. Run them on every model, prompt or search change, and track the score over time.'],
    ['Bias', 'risk', 0, 'Models absorb patterns from their training data, including unfair or one-sided ones.', 'A new hire who picked up bad habits at their last job.', 'Matters most for decisions about people: hiring, performance, lending. Test with our own cases and keep a person responsible for the decision.'],
    ['Data retention', 'risk', 0, 'How long questions, documents and answers are stored, and who can access them.', 'The difference between a conversation and a recording.', 'With a subscription it is set by the vendor’s plan and policies. When we host the model it is whatever our own policy says, including nothing at all.'],
  ];
  const glTerms = $('#glTerms'), glSearch = $('#glSearch'), glDetail = $('#glDetail');
  let glCat = 'all', glSel = 0;
  function showTerm(i) {
    glSel = i;
    const [name, cat, key, def, like, tech] = TERMS[i];
    glDetail.style.setProperty('--hl', HL[i % HL.length]);
    const likeP = el('p', 'gd-like');
    likeP.append(el('b', null, 'Like'), like);
    glDetail.replaceChildren(el('p', 'gd-cat', (key ? 'Key term · ' : '') + CATS[cat]), el('h3', null, name), el('p', 'gd-def', def), likeP,
      el('p', 'gd-tech-label', 'Under the hood'), el('p', 'gd-tech', tech));
    glDetail.classList.remove('pop'); void glDetail.offsetWidth; glDetail.classList.add('pop');
    $$('.term-chip', glTerms).forEach(c => { const on = +c.dataset.i === i; c.classList.toggle('is-on', on); c.setAttribute('aria-selected', String(on)); });
  }
  function renderTerms() {
    const q = glSearch.value.trim().toLowerCase();
    const list = TERMS.map((t, i) => [t, i]).filter(([[name, cat, , def, like, tech]]) =>
      (glCat === 'all' || cat === glCat) && (!q || (name + ' ' + def + ' ' + like + ' ' + tech).toLowerCase().includes(q)));
    glTerms.replaceChildren(...list.map(([[name, , key], i]) => {
      const b = el('button', 'term-chip' + (key ? ' key' : '') + (i === glSel ? ' is-on' : ''), name);
      b.type = 'button';
      b.dataset.i = i;
      b.setAttribute('role', 'option');
      b.setAttribute('aria-selected', String(i === glSel));
      b.addEventListener('click', () => showTerm(i));
      return b;
    }));
    if (!list.length) glTerms.append(el('p', 'gl-empty', 'No term matches. Try “token”, “RAG” or “agent”.'));
    else if (!list.some(([, i]) => i === glSel)) showTerm(list[0][1]);
  }
  $$('.gl-filters .chip').forEach(b => b.addEventListener('click', () => {
    glCat = b.dataset.cat;
    $$('.gl-filters .chip').forEach(x => x.classList.toggle('is-on', x === b));
    renderTerms();
  }));
  glSearch.addEventListener('input', renderTerms);
  renderTerms();
  showTerm(0);

  /* ------------------------------------------------------------------ 9: myths (all ten on one slide, each answer cut to its core) */
  const MYTHS = [
    ['It understands and thinks like a person.', 'It predicts likely text extremely well and can reason through problems usefully. But it has no intentions, no awareness, and no memory of you beyond what the software gives it.'],
    ['It knows everything, up to today.', 'Its knowledge stops at a training cut-off, and it has never seen anything private, like our documents. That gap is what RAG fills: the harness hands it the right documents.'],
    ['If it sounds confident, it is correct.', 'Fluency is not accuracy. Models invent figures, quotes and even sources in a confident tone. Answers built from our documents, with sources shown, are far easier to check.'],
    ['It learns from every conversation.', 'The model is frozen after training. Chat apps may remember through a separate memory feature, and some plans let vendors train on chats. A model we host learns nothing unless we retrain it.'],
    ['Open models are toys next to ChatGPT.', 'They have closed most of the gap. OpenAI’s own open-weight gpt-oss-120b (August 2025) reported near-parity with its o4-mini on core reasoning tests. Paid models still lead on the hardest problems.'],
    ['You need a data centre to run one.', 'An 8-billion-parameter model runs on an ordinary laptop, and one well-equipped server can serve a department. Our PC Companion runs on a laptop with a 12 GB graphics card, and even on its processor alone.'],
    ['To teach it our business, we must train our own model.', 'Almost never. Giving it our documents at question time (RAG) is cheaper, always up to date, and shows its sources. Fine-tuning changes style; training from scratch costs tens of crores of rupees or more.'],
    ['A bigger model always gives better answers.', 'For questions about our own business, the right documents matter more than raw size. A mid-size model with good search often beats a giant one without it, and runs faster and cheaper.'],
    ['Open-weight means open source, no strings attached.', 'The weights are downloadable; the training data usually is not, and licences differ. Some are fully permissive (Apache 2.0); Meta’s Llama licence adds conditions for very large companies. Legal should review ours.'],
    ['Once it is installed, the job is done.', 'Like any system it needs an owner: testing, moving to better models as they appear, monitoring and security updates. A modest ongoing cost that belongs in the plan from day one.'],
  ];
  const checked = new Set();
  const updateMythCount = () => $$('.myth-num').forEach(n => { n.textContent = checked.size; });
  $('.myth-grid').replaceChildren(...MYTHS.map(([myth, real], i) => {
    const b = el('button', 'myth');
    b.type = 'button';
    b.setAttribute('aria-pressed', 'false');
    b.innerHTML =
      '<span class="myth-inner">' +
      '<span class="myth-face myth-front"><span class="myth-tag">Myth</span><span class="myth-quote"></span><span class="myth-hint">Tap to check →</span></span>' +
      '<span class="myth-face myth-back" aria-hidden="true"><span class="myth-tag">Reality</span><span class="myth-real"></span></span>' +
      '</span>';
    $('.myth-quote', b).textContent = `“${myth}”`;
    $('.myth-real', b).textContent = real;
    b.addEventListener('click', () => {
      const on = !b.classList.contains('flipped');
      b.classList.toggle('flipped', on);
      b.setAttribute('aria-pressed', String(on));
      $('.myth-front', b).setAttribute('aria-hidden', String(on));
      $('.myth-back', b).setAttribute('aria-hidden', String(!on));
      checked.add(i);
      updateMythCount();
      });
      return b;
  }));
  updateMythCount();

  /* ------------------------------------------------------------------ 11: cost calculator (rupees): on-premises or AWS */
  // AWS on-demand Linux prices, Mumbai (ap-south-1), from AWS's published price list of 25 September 2026.
  const AWS_TYPES = [
    { id: 'g7e.2xlarge', gpu: '1 × RTX PRO 6000, 96 GB', usd: 5.49303 },   // same GPU memory as the on-prem example
    { id: 'g6e.2xlarge', gpu: '1 × L40S, 48 GB', usd: 2.69266 },
    { id: 'p5.4xlarge', gpu: '1 × H100, 80 GB', usd: 8.256 },
    { id: 'g6e.12xlarge', gpu: '4 × L40S, 192 GB', usd: 12.60131 },
  ];
  const cIn = { seats: $('#cSeats'), price: $('#cPrice'), hw: $('#cHw'), run: $('#cRun'), awsType: $('#awsType'), awsHours: $('#awsHours'), awsRun: $('#awsRun') };
  cIn.awsType.replaceChildren(...AWS_TYPES.map((t, i) => { const o = el('option', null, `${t.id} · ${t.gpu}`); o.value = i; return o; }));
  const calcSvg = $('#calcChart');
  let host = 'onprem';
  const moneyHtml = r => `${inr(r)} <small>(${usd(r)})</small>`;
  const niceStep = v => { const p = Math.pow(10, Math.floor(Math.log10(v))); const m = v / p; return (m <= 1 ? 1 : m <= 2 ? 2 : m <= 5 ? 5 : 10) * p; };
  function renderCalc() {
    const seats = +cIn.seats.value, price = +cIn.price.value, hw = +cIn.hw.value, run = +cIn.run.value;
    const type = AWS_TYPES[+cIn.awsType.value], hours = +cIn.awsHours.value, awsOther = +cIn.awsRun.value;
    const hourInr = type.usd * INR_PER_USD;
    $('#cSeatsOut').textContent = fmt(seats) + ' people';
    $('#cPriceOut').innerHTML = moneyHtml(price);
    $('#cHwOut').innerHTML = moneyHtml(hw);
    $('#cRunOut').innerHTML = moneyHtml(run);
    $('#awsRateOut').innerHTML = `₹${fmt(hourInr)} an hour <small>($${type.usd.toFixed(2)})</small>`;
    $('#awsHoursOut').innerHTML = `${hours} h <small>${hours >= 700 ? 'always on' : hours <= 260 ? 'office hours' : ''}</small>`;
    $('#awsRunOut').innerHTML = moneyHtml(awsOther);

    const M = 36;
    const cloudM = seats * price;
    const awsRent = hourInr * hours, awsM = awsRent + awsOther;
    const cloud36 = cloudM * M, prem36 = hw + run * M, aws36 = awsM * M;
    const aws = host === 'aws';
    const ours36 = aws ? aws36 : prem36;
    const be = cloudM > run ? hw / (cloudM - run) : Infinity;
    $('#cCloud3').innerHTML = moneyHtml(cloud36);
    $('#cOursLabel').textContent = aws ? 'AWS server, 3 years' : 'On-premises, 3 years';
    $('#cOurs3').innerHTML = moneyHtml(ours36);
    if (aws) {
      $('#cBreakLabel').textContent = 'Cheaper than subscriptions?';
      $('#cBreak').innerHTML = awsM < cloudM ? 'Yes <small>(from month 1)</small>' : 'No <small>(at these settings)</small>';
    } else {
      $('#cBreakLabel').textContent = 'Pays for itself';
      $('#cBreak').innerHTML = be <= M ? `Month ${Math.max(1, Math.ceil(be))}` : be === Infinity ? 'Never <small>(at these settings)</small>' : 'After year 3';
    }
    $('#cSave').innerHTML = cloud36 >= ours36
      ? `${inr(cloud36 - ours36)} saved <small>(${usd(cloud36 - ours36)})</small>`
      : `${inr(ours36 - cloud36)} more <small>(${usd(ours36 - cloud36)})</small>`;
    $('#calcCompare').innerHTML = `Over 3 years at these settings: on-premises <b>${both(prem36)}</b>, AWS <b>${both(aws36)}</b>, of which AWS rent is ${both(awsRent)} a month.`;
    $('#calcLegend').innerHTML =
      `<span class="key-line key-cloud"></span>Subscriptions <span class="key-line ${aws ? 'key-alt' : 'key-ours'}"></span>On-premises <span class="key-line ${aws ? 'key-ours' : 'key-alt'}"></span>AWS`;

    const W = 620, H = 300, l = 90, r = 96, t = 14, b = 30;
    const top = Math.max(cloud36, prem36, aws36) * 1.04;
    const step = niceStep(top / 4);
    const yMax = step * Math.ceil(top / step);
    const X = m => l + (m / M) * (W - l - r);
    const Y = v => t + (1 - v / yMax) * (H - t - b);
    let s = '';
    for (let v = 0; v <= yMax + 1; v += step) {
      s += `<line class="grid-line" x1="${l}" x2="${W - r}" y1="${Y(v)}" y2="${Y(v)}"/>`;
      s += `<text class="svg-label" x="${l - 8}" y="${Y(v) + 4}" text-anchor="end">${v ? inr(v) : '₹0'}</text>`;
    }
    for (const m of [0, 12, 24, 36]) {
      s += `<text class="svg-label" x="${X(m)}" y="${H - 8}" text-anchor="${m === 0 ? 'start' : m === 36 ? 'end' : 'middle'}">${m === 0 ? 'Today' : 'Year ' + m / 12}</text>`;
    }
    if (aws && awsM < cloudM) s += `<polygon class="area-save" points="${X(0)},${Y(0)} ${X(M)},${Y(cloud36)} ${X(M)},${Y(aws36)}"/>`;
    if (!aws && be < M) s += `<polygon class="area-save" points="${X(be)},${Y(hw + run * be)} ${X(M)},${Y(cloud36)} ${X(M)},${Y(prem36)}"/>`;
    const premLine = `<line class="${aws ? 'line-alt' : 'line-ours'}" x1="${X(0)}" y1="${Y(hw)}" x2="${X(M)}" y2="${Y(prem36)}"/>`;
    const awsLine = `<line class="${aws ? 'line-ours' : 'line-alt'}" x1="${X(0)}" y1="${Y(0)}" x2="${X(M)}" y2="${Y(aws36)}"/>`;
    s += `<line class="line-cloud" x1="${X(0)}" y1="${Y(0)}" x2="${X(M)}" y2="${Y(cloud36)}"/>`;
    s += aws ? premLine + awsLine : awsLine + premLine;   // the selected option is drawn on top
    if (!aws && be <= M) {
      const bx = X(be), by = Y(hw + run * be), anchor = bx > W - r - 150 ? 'end' : 'start';
      s += `<circle class="be-dot" cx="${bx}" cy="${by}" r="6"/>`;
      s += `<text class="be-label" x="${bx + (anchor === 'start' ? 12 : -12)}" y="${by - 12}" text-anchor="${anchor}">Pays for itself · month ${Math.max(1, Math.ceil(be))}</text>`;
    }
    // 3-year totals at the end of each line, nudged apart so they never overlap
    const ends = [
      { v: cloud36, cls: 'cloud' },
      { v: prem36, cls: aws ? 'alt' : 'sel' },
      { v: aws36, cls: aws ? 'sel' : 'alt' },
    ].map(e => ({ ...e, y: Y(e.v) + 4 })).sort((p, q) => p.y - q.y);
    for (let i = 1; i < ends.length; i++) ends[i].y = Math.max(ends[i].y, ends[i - 1].y + 15);
    for (const e of ends) s += `<text class="end-label ${e.cls}" x="${W - r + 8}" y="${e.y}">${inr(e.v)}</text>`;
    calcSvg.innerHTML = s;
  }
  Object.values(cIn).forEach(i => i.addEventListener('input', renderCalc));
  $$('#hostSeg .seg-btn').forEach(bt => bt.addEventListener('click', () => {
    host = bt.dataset.host;
    $$('#hostSeg .seg-btn').forEach(x => { x.classList.toggle('is-on', x === bt); x.setAttribute('aria-pressed', String(x === bt)); });
    $$('.host-fields').forEach(f => { f.hidden = f.dataset.host !== host; });
    renderCalc();
  }));
  renderCalc();

  /* ------------------------------------------------------------------ 12: departments */
  const DEPTS = [
    { name: 'Engineering', uses: ['A coding assistant that knows our private repositories', 'Explains old code before anyone changes it', 'Drafts tests, reviews and documentation'],
      keep: 'Our source code never leaves our network.', q: 'Where do we check refund limits in the billing service?',
      a: 'Refund limits are checked in the billing service’s refund validation module, before any payment call is made. The limit itself comes from the finance settings table, so changing it needs no code change.',
      src: ['billing-service › refunds › validate', 'Finance settings runbook'] },
    { name: 'Customer support', uses: ['Drafts replies from our knowledge base, with sources', 'Summarises long ticket threads in seconds', 'Spots recurring problems across hundreds of tickets'],
      keep: 'Customer details stay inside our systems.', q: 'A customer on the Pro plan says the export button is greyed out. Why?',
      a: 'On the Pro plan, exporting needs the workspace admin role, so this customer is probably a regular member. Suggest they ask their admin to change their role, or to run the export for them.',
      src: ['Knowledge base: Export permissions', 'Plan comparison 2026'] },
    { name: 'Legal and compliance', uses: ['First-pass review of contracts against our standard terms', 'Finds clauses across hundreds of agreements at once', 'Summarises new regulations and what they change for us'],
      keep: 'Contracts and legal advice stay confidential.', q: 'Which supplier contracts renew automatically next quarter?',
      a: 'Three supplier agreements renew automatically next quarter. Two of them need 60 days’ notice to cancel, and the earliest notice deadline is five weeks from today.',
      src: ['Contract register', 'Supplier agreements folder'] },
    { name: 'HR', uses: ['Answers policy questions instantly, quoting the exact clause', 'An onboarding guide for new joiners', 'Drafts job descriptions in our house style'],
      keep: 'Employee records never go to an outside provider.', q: 'How many weeks of parental leave do I get?',
      a: 'Under the current HR policy, the primary carer gets 26 weeks of paid leave and the secondary carer 4 weeks, once you have completed six months of service.',
      src: ['HR Policy 2026, section 4.2'] },
    { name: 'Sales', uses: ['First drafts of proposals from our past winning bids', 'Instant answers to product questions for the sales team', 'Call summaries and follow-up emails'],
      keep: 'Pricing and pipeline data stay private.', q: 'Draft an opening line for a proposal to a logistics client.',
      a: 'Logistics runs on timing. Our platform gives your dispatch team one live view of every order, so a delay is spotted hours earlier, not when a customer calls.',
      src: ['Winning bid: freight client 2025', 'Product one-pager'] },
    { name: 'Finance and operations', uses: ['Summarises monthly reports and explains variances', 'Reads invoices and purchase orders without sending them outside', 'Answers “where is the policy on…” questions'],
      keep: 'Financial data stays on our own servers.', q: 'Why was travel spend over budget last month?',
      a: 'Travel was 18% over budget, mainly because of two unplanned client visits and higher airfares. Leaving those two visits out, spending was 3% under budget.',
      src: ['Monthly management accounts', 'Travel expense report'] },
  ];
  const deptTabs = $('#deptTabs'), chatA = $('#chatA'), chatCaret = $('#chatCaret');
  let deptIdx = 0, typeRun = 0;
  async function typeAnswer(d, animate) {
    const me = ++typeRun;
    $('#chatQ').textContent = d.q;
    const srcBox = $('#chatSrc');
    srcBox.replaceChildren();
    const done = () => {
      chatCaret.hidden = true;
      srcBox.replaceChildren(...d.src.map((s, i) => { const c = el('span', 'src', s); c.style.animationDelay = i * 90 + 'ms'; return c; }));
    };
    if (reduced || !animate) { chatA.textContent = d.a; done(); return; }
    chatA.textContent = '';
    chatCaret.hidden = false;
    await sleep(500);
    for (const t of tokenize(d.a)) {
      if (me !== typeRun) return;
      chatA.textContent += t;
      await sleep(26);
    }
    if (me === typeRun) done();
  }
  function selectDept(i, animate = true) {
    deptIdx = i;
    const d = DEPTS[i];
    $$('[role="tab"]', deptTabs).forEach((t, j) => { t.classList.toggle('is-on', j === i); t.setAttribute('aria-selected', String(j === i)); });
    $('#deptUses').replaceChildren(...d.uses.map(u => el('li', null, u)));
    $('#deptKeep').textContent = d.keep;
    typeAnswer(d, animate);
  }
  deptTabs.replaceChildren(...DEPTS.map((d, i) => {
    const t = el('button', 'chip', d.name);
    t.type = 'button';
    t.setAttribute('role', 'tab');
    t.setAttribute('aria-controls', 'deptPanel');
    t.addEventListener('click', () => selectDept(i));
    return t;
  }));
  selectDept(0, false);
  hooks.value = () => typeAnswer(DEPTS[deptIdx], true);

  /* ------------------------------------------------------------------ 13: harness rings (layers light up in turn) */
  const RINGS = [['audit', 'Audit log'], ['guard', 'Rules'], ['perm', 'Access rights'], ['tools', 'Tools'], ['rag', 'Our documents']];
  function ringsSvg(onKeys) {
    const c = 180, gap = 26, core = 40, capH = 8.5;
    let s = '';
    RINGS.forEach(([key, name], i) => {
      const r = 174 - i * gap;
      const on = onKeys.includes(key);
      // each label curves along the band just inside its ring, centred between that ring and the next one in
      const inner = i < RINGS.length - 1 ? r - gap : core;
      const R = inner + (r - inner - capH) / 2;
      s += `<circle class="ring${on ? ' on' : ''}" cx="${c}" cy="${c}" r="${r}"/>`;
      s += `<path id="ring-arc-${key}" fill="none" d="M ${c - R} ${c} A ${R} ${R} 0 0 1 ${c + R} ${c}"/>`;
      s += `<text class="ring-label${on ? ' on' : ''}"><textPath href="#ring-arc-${key}" startOffset="50%" text-anchor="middle">${name}</textPath></text>`;
    });
    s += `<circle class="core" cx="${c}" cy="${c}" r="40"/>`;
    s += `<text class="core-text" x="${c}" y="${c - 2}" text-anchor="middle">The</text>`;
    s += `<text class="core-text" x="${c}" y="${c + 15}" text-anchor="middle">model</text>`;
    return s;
  }
  const ringsIntro = $('#ringsIntro');
  ringsIntro.innerHTML = ringsSvg(RINGS.map(r => r[0]));
  hooks.harness = async () => {
    if (reduced) return;
    const lit = [];
    ringsIntro.innerHTML = ringsSvg(lit);
    for (const [key] of [...RINGS].reverse()) {
      await sleep(420);
      if (!isActive('harness')) { ringsIntro.innerHTML = ringsSvg(RINGS.map(r => r[0])); return; }
      lit.push(key);
      ringsIntro.innerHTML = ringsSvg(lit);
    }
  };

  /* BUILD:rag START */
  /* ------------------------------------------------------------------ 14: RAG pipeline and ChromaDB ingestion */
  /* BUILD:rag END */

  /* BUILD:agents START */
  /* ------------------------------------------------------------------ 15: multi-agent orchestration */
  /* BUILD:agents END */

  /* BUILD:hitl START */
  /* ------------------------------------------------------------------ 16: human in the loop */
  /* BUILD:hitl END */

  /* BUILD:observe START */
  /* ------------------------------------------------------------------ 17: LangSmith observability */
  /* BUILD:observe END */

  /* ------------------------------------------------------------------ 19: four routes */
  const PATHS = [
    { title: 'A · Use an open-weight model as it is',
      desc: 'Download a strong open model, run it on our own server and give people a private chat window. Everything stays private, but the model only knows what it learned in training. It knows nothing about our company.',
      time: 'Days', good: 'Drafting, summarising, translation, general coding help',
      watch: 'Cannot answer anything about our own documents, policies or data', take: [1, 1, 1, 0], get: [0, 0, 0, 4] },
    { title: 'B · Open-weight model + RAG',
      desc: 'Route A, plus a search layer over our documents. For every question the harness finds the relevant passages, gives them to the model and has it answer from them, with sources. Change a document and the next answer reflects it.',
      time: '4–8 weeks to a pilot', good: 'Questions about our policies, products, contracts and code, with sources',
      watch: 'Only as good as our documents and the search behind them; needs tuning and an owner', take: [2, 2, 2, 2], get: [5, 5, 5, 5] },
    { title: 'C · Fine-tune an open-weight model',
      desc: 'Take an open model and keep training it on our own examples, so its behaviour, tone or format matches what we want. Useful for narrow, repeated jobs. It does not reliably learn facts, and what it learns freezes the day training ends.',
      time: '2–4 months a round, repeated when things change', good: 'A fixed style or format; narrow tasks such as sorting tickets',
      watch: 'Knowledge frozen at training time, no sources, and every policy change means retraining', take: [3, 3, 4, 5], get: [2, 1, 0, 5] },
    { title: 'D · Train our own model from scratch',
      desc: 'Build a new model from nothing: gather trillions of words of text, rent thousands of GPUs for months and hire a research team to run it. This is what OpenAI, Anthropic, Google and Meta do.',
      time: 'A year or more', good: 'Companies whose product is the model itself',
      watch: 'Needs far more text than we own; the result would almost certainly be worse than free open-weight models', take: [5, 5, 5, 5], get: [2, 1, 0, 5] },
  ];
  const TAKE = ['Cost', 'Time', 'Team needed', 'Data preparation'];
  const GET = ['Knows our documents', 'Stays up to date', 'Shows its sources', 'Control'];
  const pathCards = $$('.path-card');
  const meters = $('#pathMeters');
  const meterGroup = (title, cls, labels) => {
    const g = el('div', 'meter-group ' + cls);
    g.append(el('p', 'meter-group-title', title));
    labels.forEach(lb => {
      const row = el('div', 'mrow');
      row.setAttribute('role', 'img');
      const segs = el('span', 'segs');
      for (let k = 0; k < 5; k++) { const s = el('i'); s.style.setProperty('--k', k); segs.append(s); }
      row.append(el('span', null, lb), segs);
      g.append(row);
    });
    return g;
  };
  meters.append(meterGroup('What it takes', 'take', TAKE), meterGroup('What we get', 'get', GET));
  function selectPath(i) {
    const p = PATHS[i];
    pathCards.forEach((c, j) => { c.classList.toggle('is-on', j === i); c.setAttribute('aria-selected', String(j === i)); });
    $('#pathTitle').textContent = p.title;
    $('#pathDesc').textContent = p.desc;
    $('#pathTime').textContent = p.time;
    $('#pathGood').textContent = p.good;
    $('#pathWatch').textContent = p.watch;
    [['.take', p.take, TAKE], ['.get', p.get, GET]].forEach(([sel, vals, labels]) => {
      $$(sel + ' .mrow', meters).forEach((row, r) => {
        row.setAttribute('aria-label', `${labels[r]}: ${vals[r]} out of 5`);
        $$('i', row).forEach((s, k) => s.classList.toggle('on', k < vals[r]));
      });
    });
  }
  pathCards.forEach((c, i) => c.addEventListener('click', () => selectPath(i)));
  selectPath(1);

  /* BUILD:polish START */
  /* ------------------------------------------------------------------ deck-wide polish */
  // Entrances are CSS keyed to .slide.on (styles.css, BUILD:polish). This adds what CSS cannot do alone: decorative
  // layers, a few measurements, the cost chart drawing itself, clickable harness rings, the four-part bar and the
  // slide list. Everything is skipped or shown in its final state under reduced motion and in flow (phone) mode.
  const SVGNS = 'http://www.w3.org/2000/svg';
  const svgEl = (tag, attrs = {}) => { const n = document.createElementNS(SVGNS, tag); for (const k in attrs) n.setAttribute(k, attrs[k]); return n; };
  const motionOn = () => !reduced && isDeck();
  const chain = (id, fn) => { const prev = hooks[id]; hooks[id] = () => fn(prev); };
  const frameOf = id => $(`#${id} > .frame`);
  // position of a node inside its slide's 1600 × 860 canvas, from layout offsets (unaffected by any transform)
  const canvasPos = n => { let x = 0, y = 0; for (let p = n; p && !p.classList?.contains('frame'); p = p.offsetParent) { x += p.offsetLeft; y += p.offsetTop; } return { x, y, w: n.offsetWidth, h: n.offsetHeight }; };

  // .pz-in marks a slide for a moment after it arrives, for entrances that must not replay on every click
  const pzTimers = new Map();
  const classWatch = new MutationObserver(recs => {
    for (const r of recs) {
      const s = r.target, on = s.classList.contains('on'), was = (r.oldValue || '').split(/\s+/).includes('on');
      if (on === was) continue;
      clearTimeout(pzTimers.get(s));
      s.classList.toggle('pz-in', on);
      if (on) pzTimers.set(s, setTimeout(() => s.classList.remove('pz-in'), 2600));
    }
  });
  slides.forEach(s => classWatch.observe(s, { attributes: true, attributeFilter: ['class'], attributeOldValue: true }));

  /* finish: a slow drifting glow behind every dark slide */
  $$('.slide.dark').forEach(s => { const g = el('div', 'pz-glow'); g.setAttribute('aria-hidden', 'true'); s.prepend(g); });

  /* finish: a soft sheen that follows the cursor across plain cards */
  let sheenCard = null, sheenXY = null, sheenRaf = 0;
  document.addEventListener('pointermove', e => {
    if (!isDeck() || e.pointerType === 'touch') return;
    const c = e.target.closest?.('.slide.on .card') || null;
    if (c && !c.dataset.pzSheen) {
      c.dataset.pzSheen = getComputedStyle(c).backgroundImage === 'none' ? 'on' : 'off';
      c.classList.toggle('pz-sheen', c.dataset.pzSheen === 'on');
    }
    sheenCard = c && c.classList.contains('pz-sheen') ? c : null;
    if (!sheenCard) return;
    sheenXY = [e.clientX, e.clientY];
    if (!sheenRaf) sheenRaf = requestAnimationFrame(() => {
      sheenRaf = 0;
      if (!sheenCard || !sheenXY) return;
      const r = sheenCard.getBoundingClientRect();
      sheenCard.style.setProperty('--pz-mx', Math.round((sheenXY[0] - r.left) / S) + 'px');
      sheenCard.style.setProperty('--pz-my', Math.round((sheenXY[1] - r.top) / S) + 'px');
    });
  }, { passive: true });

  /* 1 title: a quiet field of candidate tokens floating in the clear space above and below the live demo */
  const FIELD_WORDS = [' our', ' private', ' servers', ' inside', ' network', ' rules', ' ours',
    ' documents', ' own', ' hardware', ' knowledge', ' secure'];
  const topFrame = frameOf('top'), genCard = $('#genDemo');
  const field = el('div', 'pz-field');
  field.setAttribute('aria-hidden', 'true');
  const fieldToks = FIELD_WORDS.map((w, k) => {
    const t = el('span', 'pz-tok', w);
    t.style.background = HL[(k * 2) % HL.length];
    t.style.fontSize = [16, 18, 15, 17][k % 4] + 'px';
    t.style.setProperty('--fx', (6 + (k * 5) % 8) + 'px');
    t.style.setProperty('--fy', (3 + (k * 3) % 4) + 'px');
    t.style.setProperty('--dur', (9 + (k * 7) % 8) + 's');
    t.style.setProperty('--bdur', (5 + (k * 3) % 5) + 's');
    t.style.setProperty('--del', -((k * 1.7) % 9).toFixed(1) + 's');
    return t;
  });
  field.append(...fieldToks);
  topFrame?.prepend(field);
  function placeField() {
    if (!isDeck() || !genCard || !topFrame) return;
    const c = canvasPos(genCard), rnd = seeded(5171);
    const x0 = c.x + 4, x1 = c.x + c.w - 4;
    // clear space above and below the card; the demo text can grow by a line, so keep well clear of its edges
    const bands = [[66, c.y - 56], [c.y + c.h + 54, 860 - 52 - 10]];
    const placed = [];
    fieldToks.forEach((t, k) => {
      t.hidden = false;
      const w = t.offsetWidth || 90, h = t.offsetHeight || 31;
      t.hidden = true;
      for (let tries = 0; tries < 80; tries++) {
        const [y0, y1] = bands[(k + tries) % 2];
        if (y1 - y0 < h) continue;
        const x = x0 + rnd() * (x1 - x0 - w), y = y0 + rnd() * (y1 - y0 - h);
        // the float moves each chip by up to 14 × 7 px either way: keep a gap twice that
        if (placed.some(p => x < p.x + p.w + 44 && x + w + 44 > p.x && y < p.y + p.h + 22 && y + h + 22 > p.y)) continue;
        placed.push({ x, y, w, h });
        t.style.left = Math.round(x) + 'px';
        t.style.top = Math.round(y) + 'px';
        t.hidden = false;
        break;
      }
    });
  }
  chain('top', prev => { prev?.(); placeField(); });

  /* 3 learn: "15 trillion+" counts up */
  {
    const b = $('#learn .stage-stat b'), t = b?.firstChild;
    const m = t?.nodeType === 3 ? t.nodeValue.match(/^(\d+)([\s\S]*)$/) : null;
    if (m) { const n = el('span', null, m[1]); n.dataset.count = m[1]; t.replaceWith(n, m[2]); }
  }

  /* 4 work: the prompt splits into its six tokens one after another */
  $$('#work .frame-v[data-frame="1"] .tk-chip').forEach((c, k) => c.style.setProperty('--pz-k', k));

  /* 7 frozen: frost crystallises in the empty corner while the points freeze into place */
  const frost = (() => {
    const frame = frameOf('frozen');
    if (!frame) return null;
    const W = 660, H = 400, rnd = seeded(20260929);
    const svg = svgEl('svg', { class: 'pz-frost', viewBox: `0 0 ${W} ${H}`, 'aria-hidden': 'true' });
    const defs = svgEl('defs');
    const fade = svgEl('radialGradient', { id: 'pzFrostFade', gradientUnits: 'userSpaceOnUse', cx: W, cy: 0, r: 640 });
    [[0, 1], [0.7, 0.8], [1, 0]].forEach(([o, a]) => fade.append(svgEl('stop', { offset: o, 'stop-color': '#fff', 'stop-opacity': a })));
    const mask = svgEl('mask', { id: 'pzFrostMask', maskUnits: 'userSpaceOnUse', x: 0, y: 0, width: W, height: H });
    mask.append(svgEl('rect', { x: 0, y: 0, width: W, height: H, fill: 'url(#pzFrostFade)' }));
    defs.append(fade, mask);
    const g = svgEl('g', { class: 'pz-frost-g', mask: 'url(#pzFrostMask)' });
    const tips = [];
    // keep every stroke inside the box so nothing pokes out of the canvas
    const clip = (x, y, dx, dy) => {
      let t = 1;
      if (x + dx < 2) t = Math.min(t, (2 - x) / dx);
      if (x + dx > W - 2) t = Math.min(t, (W - 2 - x) / dx);
      if (y + dy < 2) t = Math.min(t, (2 - y) / dy);
      if (y + dy > H - 2) t = Math.min(t, (H - 2 - y) / dy);
      return Math.max(0, t);
    };
    const paths = [[], [], []];
    const grow = (x, y, ang, len, depth) => {
      const dx = Math.cos(ang) * len, dy = Math.sin(ang) * len, t = clip(x, y, dx, dy);
      if (t * len < 6) return;
      const ex = x + dx * t, ey = y + dy * t;
      paths[depth].push(`M${x.toFixed(1)} ${y.toFixed(1)}L${ex.toFixed(1)} ${ey.toFixed(1)}`);
      if (depth === 2 || (depth === 1 && t * len < 44)) return;
      // ice grows in sixty-degree side branches that shorten towards the tip
      const step = depth ? 19 : 31, n = Math.floor((t * len) / step);
      for (let i = 1; i <= n; i++) {
        const f = (i - 0.35) / n, bx = x + dx * t * f, by = y + dy * t * f;
        const bl = len * (depth ? 0.34 : 0.33) * (1 - f * 0.78) * (0.8 + rnd() * 0.4);
        grow(bx, by, ang - Math.PI / 3, bl, depth + 1);
        grow(bx, by, ang + Math.PI / 3, bl * (0.8 + rnd() * 0.3), depth + 1);
      }
      if (depth === 0) tips.push([ex, ey]);
      else if (rnd() < 0.18) tips.push([ex, ey]);
    };
    const deg = Math.PI / 180;
    [[W - 26, 0, 124, 300], [W - 250, 0, 101, 190], [W, 196, 172, 250], [W - 430, 0, 86, 104]]
      .forEach(([x, y, a, l]) => grow(x, y, a * deg, l, 0));
    paths.forEach((ds, depth) => g.append(svgEl('path', { class: 'pz-fr', d: ds.join(''), 'stroke-width': [1.5, 1, 0.75][depth], opacity: [0.6, 0.46, 0.34][depth] })));
    tips.slice(0, 14).forEach(([x, y], k) => {
      const s = 3 + (k % 3);
      const sp = svgEl('path', { class: 'pz-spark', d: `M${x} ${y - s * 2}L${x + s * 0.45} ${y - s * 0.45}L${x + s * 2} ${y}L${x + s * 0.45} ${y + s * 0.45}L${x} ${y + s * 2}L${x - s * 0.45} ${y + s * 0.45}L${x - s * 2} ${y}L${x - s * 0.45} ${y - s * 0.45}Z` });
      sp.style.setProperty('--tw', (2.6 + (k % 4) * 0.7) + 's');
      sp.style.setProperty('--twd', (1.8 + (k * 0.37) % 2.4).toFixed(2) + 's');
      g.append(sp);
    });
    svg.append(defs, g);
    frame.prepend(svg);
    return { fade, R: 640 };
  })();
  let frostRun = 0;
  chain('frozen', prev => {
    prev?.();
    if (!frost) return;
    const me = ++frostRun;
    if (!motionOn()) { frost.fade.setAttribute('r', frost.R); return; }
    frost.fade.setAttribute('r', 1);
    const t0 = performance.now();
    const tick = now => {
      if (me !== frostRun) return;
      const k = clamp((now - t0 - 350) / 2100);
      frost.fade.setAttribute('r', Math.max(1, frost.R * (1 - Math.pow(1 - k, 2.2))).toFixed(1));
      if (k < 1 && isActive('frozen')) requestAnimationFrame(tick); else frost.fade.setAttribute('r', frost.R);
    };
    requestAnimationFrame(tick);
  });

  /* 9 myths: work out where each card is dealt from (a point just below the middle of the table) */
  function measureDeal() {
    const grid = $('.myth-grid');
    if (!grid || !isDeck()) return;
    const gp = canvasPos(grid), ox = gp.w / 2, oy = gp.h + 40;
    $$('.myth', grid).forEach((c, k) => {
      const p = canvasPos(c), cx = p.x - gp.x + p.w / 2, cy = p.y - gp.y + p.h / 2;
      c.style.setProperty('--pz-k', k);
      c.style.setProperty('--pz-dx', Math.round((ox - cx) * 0.42) + 'px');
      c.style.setProperty('--pz-dy', Math.round((oy - cy) * 0.42) + 'px');
      c.style.setProperty('--pz-rot', ((k % 2 ? 1 : -1) * (4 + ((k * 7) % 5))) + 'deg');
    });
  }
  chain('myths', prev => { prev?.(); measureDeal(); });

  /* 8 terms: number the chips so they cascade in reading order */
  chain('terms', prev => { prev?.(); $$('#glTerms .term-chip').forEach((c, k) => c.style.setProperty('--pz-k', k)); });

  /* 11 cost: the pay-back label sits where two lines cross; give it a paper plate so no line shows between its words */
  function plateBreakEven() {
    const lab = $('.be-label', calcSvg);
    $('.pz-be-plate', calcSvg)?.remove();
    if (!lab) return;
    const b = lab.getBBox();
    if (!b.width) return;
    lab.before(svgEl('rect', { class: 'pz-be-plate', x: (b.x - 7).toFixed(1), y: (b.y - 3).toFixed(1), width: (b.width + 14).toFixed(1), height: (b.height + 6).toFixed(1), rx: 6 }));
  }
  new MutationObserver(recs => { if (recs.some(r => [...r.addedNodes].some(n => !n.classList?.contains('pz-be-plate')))) plateBreakEven(); })
    .observe(calcSvg, { childList: true });
  plateBreakEven();
  document.fonts?.ready.then(plateBreakEven);

  /* 11 cost: the three lines draw themselves, then the savings area and the pay-back point appear */
  let costRun = 0;
  function drawCostIn() {
    const me = ++costRun;
    const plan = [['.line-cloud', '.end-label.cloud'], ['.line-ours', '.end-label.sel'], ['.line-alt', '.end-label.alt']]
      .map(([ls, es], k) => ({ ln: $(ls, calcSvg), end: $(es, calcSvg), start: 620 + k * 170, dur: 900 }))
      .filter(p => p.ln);
    plan.forEach(p => {
      p.a = ['x1', 'y1', 'x2', 'y2'].map(n => +p.ln.getAttribute(n));
      p.ln.setAttribute('x2', p.a[0]);
      p.ln.setAttribute('y2', p.a[1]);
      p.end?.animate({ opacity: [0, 1] }, { duration: 400, delay: p.start + p.dur - 120, easing: 'ease-out', fill: 'backwards' });
    });
    $$('.area-save', calcSvg).forEach(n => n.animate({ opacity: [0, getComputedStyle(n).opacity] }, { duration: 600, delay: 1650, easing: 'ease-out', fill: 'backwards' }));
    $$('.be-dot, .pz-be-plate, .be-label', calcSvg).forEach((n, k) => n.animate({ opacity: [0, 1] }, { duration: 420, delay: 1720 + k * 80, easing: 'ease-out', fill: 'backwards' }));
    const t0 = performance.now();
    const ease = k => (k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2);
    const tick = now => {
      const live = me === costRun && isActive('cost');
      let done = true;
      for (const p of plan) {
        if (!p.ln.isConnected) continue;
        const k = live ? clamp((now - t0 - p.start) / p.dur) : 1;
        if (k < 1) done = false;
        p.ln.setAttribute('x2', (p.a[0] + (p.a[2] - p.a[0]) * ease(k)).toFixed(2));
        p.ln.setAttribute('y2', (p.a[1] + (p.a[3] - p.a[1]) * ease(k)).toFixed(2));
      }
      if (!done) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }
  chain('cost', prev => { prev?.(); if (motionOn()) drawCostIn(); });

  /* 12 value: the department panel arrives after the two numbers have counted up, then starts typing */
  let valueRun = 0;
  chain('value', prev => {
    const me = ++valueRun;
    if (!motionOn()) { prev?.(); return; }
    setTimeout(() => { if (me === valueRun && isActive('value')) prev?.(); }, 900);
  });

  /* 13 harness: each ring links to the slide that opens it up, without changing how the rings look */
  const RING_TO = { rag: 'rag', perm: 'rag', tools: 'agents', guard: 'hitl', audit: 'observe' };
  const ringHint = el('p', 'pz-ring-hint rv');
  ringHint.style.setProperty('--d', 3);
  ringHint.setAttribute('aria-live', 'polite');
  const HINT_ICON = '<svg viewBox="0 0 20 20" aria-hidden="true"><circle cx="10" cy="10" r="8.6"/><circle cx="10" cy="10" r="4.6"/></svg>';
  const restHint = () => { ringHint.innerHTML = HINT_ICON; ringHint.append(el('span', null, 'The next four slides open these rings. Click a ring to go there.')); };
  const ringLinks = svgEl('svg', { class: 'pz-rings rv', viewBox: '0 0 360 360', role: 'group', 'aria-label': 'The harness rings' });
  ringLinks.style.setProperty('--d', 3);
  RINGS.forEach(([key, name], i) => {
    const target = document.getElementById(RING_TO[key]);
    if (!target) return;
    // ring geometry mirrors ringsSvg(): radius 174 − 26 per ring, the model's core is 40
    const r = 174 - i * 26, inner = i < RINGS.length - 1 ? r - 26 : 40;
    const a = svgEl('a', { href: '#' + target.id, 'aria-label': `${name}: go to ${target.dataset.label}` });
    a.append(svgEl('circle', { class: 'pz-band', cx: 180, cy: 180, r: (r + inner) / 2, 'stroke-width': r - inner }));
    const show = () => {
      ringHint.innerHTML = HINT_ICON;
      const s = el('span');
      s.append(el('b', null, name), ` → ${target.dataset.label}`);
      ringHint.append(s);
    };
    a.addEventListener('pointerenter', show);
    a.addEventListener('focus', show);
    a.addEventListener('pointerleave', restHint);
    a.addEventListener('blur', restHint);
    ringLinks.append(a);
  });
  ringsIntro.before(ringLinks);
  $('#harness .hi-text')?.append(ringHint);
  restHint();

  /* 19 routes: number the meter rows so they fill top to bottom */
  $$('#pathMeters .mrow').forEach((r, k) => r.style.setProperty('--pz-r', k));

  /* 20 plan: the closing line is split into tokens that write themselves with the highlighter rhythm */
  {
    const line = $('#plan .close-line');
    if (line) {
      let i = 0;
      const frag = document.createDocumentFragment();
      for (const t of tokenize(line.textContent)) {
        const lead = t.match(/^\s*/)[0];
        if (lead) frag.append(lead);
        if (lead === t) continue;
        const s = el('span', 'tk', t.slice(lead.length));
        s.style.setProperty('--i', i);
        s.style.setProperty('--hl', HL[i++ % HL.length]);
        frag.append(s);
      }
      line.replaceChildren(frag);
    }
  }

  /* bottom bar: the four parts of the talk on the progress track, read from the title slide's agenda */
  const PZ_PARTS = (() => {
    const parts = $$('.hero-agenda a[href^="#"]').map(a => ({
      jump: slides.findIndex(s => s.id === a.getAttribute('href').slice(1)),
      name: [...a.childNodes].filter(n => n.nodeType === 3).map(n => n.nodeValue).join('').trim() || a.textContent.trim(),
    })).filter(p => p.jump >= 0).sort((p, q) => p.jump - q.jump);
    if (!parts.length) parts.push({ jump: 0, name: 'The talk' });
    parts.forEach((p, k) => { p.from = k ? p.jump : 0; });
    parts.forEach((p, k) => { p.to = k < parts.length - 1 ? parts[k + 1].from - 1 : slides.length - 1; });
    return parts;
  })();
  const partOf = i => Math.max(0, PZ_PARTS.findIndex(p => i >= p.from && i <= p.to));
  const COUNT_WORDS = ['One', 'Two', 'Three', 'Four', 'Five', 'Six', 'Seven', 'Eight'];
  const partName = k => `Part ${k + 1} of ${PZ_PARTS.length} · ${PZ_PARTS[k].name}`;
  const slideName = s => (s.dataset.label || s.id).replace(/^\d+\s*·\s*/, '');
  const pzTrack = el('div', 'pz-track');
  const pzLabel = el('p', 'pz-label');
  const pzSegs = el('div', 'pz-segs');
  pzSegs.setAttribute('role', 'group');
  pzSegs.setAttribute('aria-label', 'Parts of the talk');
  let peek = -1;
  const segBtns = PZ_PARTS.map((p, k) => {
    const b = el('button', 'pz-seg');
    b.type = 'button';
    b.style.flexGrow = p.to - p.from + 1;
    b.setAttribute('aria-label', `Go to part ${k + 1} of ${PZ_PARTS.length}: ${p.name}, slides ${p.from + 1} to ${p.to + 1}`);
    const track = el('span', 'pz-seg-bar');
    track.append(el('span', 'pz-seg-fill'));
    b.append(track);
    b.addEventListener('click', () => go(p.jump));
    b.addEventListener('pointerenter', () => { peek = k; paintParts(); });
    b.addEventListener('pointerleave', () => { peek = -1; paintParts(); });
    b.addEventListener('focus', () => { peek = k; paintParts(); });
    b.addEventListener('blur', () => { peek = -1; paintParts(); });
    return b;
  });
  pzSegs.append(...segBtns);
  pzTrack.append(pzLabel, pzSegs);
  $('#barTrack').after(pzTrack);

  // the slide counter opens the slide list
  const barCount = $('#barCount');
  const countBtn = el('button', 'pz-count');
  countBtn.type = 'button';
  countBtn.setAttribute('aria-haspopup', 'dialog');
  countBtn.setAttribute('aria-expanded', 'false');
  countBtn.setAttribute('aria-controls', 'pzNav');
  countBtn.innerHTML = '<svg class="pz-count-ico" viewBox="0 0 14 14" aria-hidden="true"><rect width="6" height="6" rx="1.5"/><rect x="8" width="6" height="6" rx="1.5"/><rect y="8" width="6" height="6" rx="1.5"/><rect x="8" y="8" width="6" height="6" rx="1.5"/></svg>';
  barCount.replaceWith(countBtn);
  countBtn.append(barCount);

  const nav = $('#pzNav'), navGrid = $('#pzNavGrid');
  $('#pzNavTitle').textContent = `All ${slides.length} slides`;
  navGrid.replaceChildren(...PZ_PARTS.map((p, k) => {
    const sec = el('section', 'pz-nav-part');
    sec.append(el('p', 'pz-nav-pn', `Part ${k + 1}`), el('p', 'pz-nav-pt', p.name));
    const list = el('ol', 'pz-nav-list');
    for (let i = p.from; i <= p.to; i++) {
      const a = el('a', 'pz-nav-item');
      a.setAttribute('href', '#' + slides[i].id);
      a.dataset.i = i;
      a.append(el('span', 'pz-nav-n', String(i + 1).padStart(2, '0')), el('span', 'pz-nav-t', slideName(slides[i])));
      const li = el('li');
      li.append(a);
      list.append(li);
    }
    sec.append(list);
    return sec;
  }));
  const navItems = $$('.pz-nav-item', nav);
  const navOpen = () => !nav.hidden;
  let navBack = null;
  function markNav() {
    navItems.forEach(a => { if (+a.dataset.i === cur) a.setAttribute('aria-current', 'page'); else a.removeAttribute('aria-current'); });
    $$('.pz-nav-part', nav).forEach((s, k) => s.classList.toggle('is-cur', k === partOf(cur)));
  }
  function openNav() {
    if (!isDeck() || navOpen()) return;
    markNav();
    navBack = document.activeElement;
    nav.hidden = false;
    countBtn.setAttribute('aria-expanded', 'true');
    navItems.find(a => +a.dataset.i === cur)?.focus({ preventScroll: true });
  }
  function closeNav(jumped = false) {
    if (!navOpen()) return;
    nav.hidden = true;
    countBtn.setAttribute('aria-expanded', 'false');
    const back = !jumped && navBack?.isConnected && navBack !== document.body ? navBack : countBtn;
    back.focus?.({ preventScroll: true });
  }
  countBtn.addEventListener('click', () => (navOpen() ? closeNav() : openNav()));
  // an item is a plain #link: the deck's own link handler moves to the slide; this only closes the list
  nav.addEventListener('click', e => {
    if (e.target.closest('[data-pz-close]')) closeNav();
    else if (e.target.closest('.pz-nav-item')) closeNav(true);
  });
  const partStarts = PZ_PARTS.map(p => navItems.findIndex(a => +a.dataset.i === p.from));
  addEventListener('keydown', e => {
    const mods = e.ctrlKey || e.metaKey || e.altKey;
    if (navOpen()) {
      const i = navItems.indexOf(document.activeElement);
      const to = j => navItems[clamp(j, 0, navItems.length - 1)].focus();
      const across = d => { // same row in the neighbouring part
        const k = partOf(i < 0 ? cur : +navItems[i].dataset.i), row = i < 0 ? 0 : i - partStarts[k];
        const n = clamp(k + d, 0, PZ_PARTS.length - 1), p = PZ_PARTS[n];
        to(partStarts[n] + Math.min(row, p.to - p.from));
      };
      let handled = !mods;
      if (handled) switch (e.key) {
        case 'Escape': case 'g': case 'G': closeNav(); break;
        case 'ArrowDown': to(i + 1); break;
        case 'ArrowUp': to(i < 0 ? 0 : i - 1); break;
        case 'ArrowRight': across(1); break;
        case 'ArrowLeft': across(-1); break;
        case 'Home': case 'PageUp': to(0); break;
        case 'End': case 'PageDown': to(navItems.length - 1); break;
        case ' ': if (i >= 0) navItems[i].click(); else handled = false; break;
        case 'Tab': {
          const f = $$('button, a', nav), j = f.indexOf(document.activeElement);
          f[((e.shiftKey ? j - 1 : j + 1) + f.length) % f.length].focus();
          break;
        }
        default: handled = false;
      }
      if (e.key === 'Escape') handled = true;
      if (handled) { e.preventDefault(); e.stopImmediatePropagation(); }
      return;
    }
    if ((e.key === 'g' || e.key === 'G') && !mods && isDeck() && !e.target.closest?.('input, textarea, select, [contenteditable]')) {
      e.preventDefault();
      openNav();
    }
  }, true);
  // while the list is open the wheel belongs to it, not to the deck
  addEventListener('wheel', e => { if (navOpen()) e.stopImmediatePropagation(); }, { capture: true, passive: true });

  function paintParts() {
    const k = partOf(cur);
    segBtns.forEach((b, j) => {
      const p = PZ_PARTS[j];
      b.style.setProperty('--f', cur > p.to ? 1 : cur < p.from ? 0 : (cur - p.from + 1) / (p.to - p.from + 1));
      b.classList.toggle('is-peek', j === peek);
      if (j === k) b.setAttribute('aria-current', 'step'); else b.removeAttribute('aria-current');
    });
    pzLabel.classList.toggle('is-peek', peek >= 0);
    pzLabel.textContent = peek >= 0 ? `Go to part ${peek + 1} · ${PZ_PARTS[peek].name}`
      : cur === 0 ? `${COUNT_WORDS[PZ_PARTS.length - 1] || PZ_PARTS.length} parts` : partName(k);
    countBtn.setAttribute('aria-label', `Slide ${cur + 1} of ${slides.length}. Open the slide list (G)`);
    if (navOpen()) markNav();
  }
  // updateBar() rewrites the counter on every move: follow it rather than touching go()
  new MutationObserver(paintParts).observe(barCount, { childList: true, characterData: true, subtree: true });
  paintParts();

  // layout-dependent pieces are measured in canvas px; redo them when fonts arrive or the mode changes
  const remeasure = () => { if (isDeck()) { placeField(); measureDeal(); } else closeNav(); };
  document.fonts?.ready.then(remeasure);
  let pzResize = 0;
  addEventListener('resize', () => { clearTimeout(pzResize); pzResize = setTimeout(remeasure, 200); });
  /* BUILD:polish END */

  /* ------------------------------------------------------------------ start */
  function relayout() {
    const modeChanged = fit();
    drawAttention();
    if (modeChanged) {
      slides.forEach(s => s.classList.remove('on'));
      visible.clear();
      if (isDeck()) go(cur, true); else slides[cur].scrollIntoView();
    }
  }
  let resizeT = 0;
  addEventListener('resize', () => { clearTimeout(resizeT); resizeT = setTimeout(relayout, 120); });
  document.fonts?.ready.then(drawAttention);

  fit();
  const startId = location.hash.slice(1);
  cur = Math.max(0, slides.findIndex(s => s.id === startId));
  if (isDeck()) go(cur, true);
  else if (cur) slides[cur].scrollIntoView();
  updateBar();
})();
