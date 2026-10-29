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
  /* ------------------------------------------------------------------ 6: temperature and top-K */
  // Chances = softmax(score ÷ T); top-K then keeps the K most likely words and rescales them to add up to 100%.
  // The last three words are the "hallucinations": plausible-sounding, wrong. On arrival a short demo sweeps the
  // dials from Facts to Brainstorm to Chat, drawing 12 answers on the way; touching anything hands over control.
  const CTL_WORDS = [['safe', 2.3], ['private', 2.0], ['secure', 1.7], ['protected', 1.4], ['yours', 0.9], ['moving', -0.2], ['guessing', -0.9], ['in a volcano', -2.6]];
  const CTL_ODD = 5;                       // index of the first nonsense word
  const CTL_PRESETS = { facts: [0.2, 1], chat: [0.7, 5], brainstorm: [1.3, 8] };
  const CTL_MAX = 0.15, CTL_SWEEP = 220;   // gauge: 0 to 15% over a 220° arc, square-root scale so the low band is readable
  const CTL_BANDS = [0.01, 0.05];          // low below 1%, medium to 5%, high above
  const ctlT = $('#ctlT'), ctlK = $('#ctlK'), ctlBars = $('#ctlBars'), ctlGauge = $('#ctlGauge'), ctlChips = $('#ctlChips');
  const ctlPresetBtns = $$('#ctlPresets .seg-btn');
  const ctl = { T: 0.7, K: 5 };
  let ctlDrawnAt = '', ctlTouched = false, ctlDemo = 0, ctlMove = 0, ctlCutK = 0, ctlCutTimer = 0;
  const ctlPct = x => (x > 0 && x < 0.005 ? '<1%' : Math.round(x * 100) + '%');
  // whole percentages for the kept words that add up to exactly 100 (largest remainder), so the column checks out
  const ctlWhole = xs => {
    const raw = xs.map(x => x * 100), out = raw.map(Math.floor);
    let left = 100 - out.reduce((a, b) => a + b, 0);
    raw.map((x, i) => [x - out[i], i]).sort((a, b) => b[0] - a[0]).forEach(([, i]) => { if (left > 0) { out[i]++; left--; } });
    return out.map((n, i) => (n === 0 && xs[i] > 0 ? '<1%' : n + '%'));
  };
  const ctlKey = (T, K) => `${T.toFixed(1)}|${K}`;
  const ctlKLabel = K => (K >= CTL_WORDS.length ? 'all' : String(K));
  function ctlOdds(T, K) {
    const p = softmax(CTL_WORDS.map(w => w[1]), T);
    const kept = p.slice(0, K).reduce((a, b) => a + b, 0);
    const q = p.map((x, i) => (i < K ? x / kept : 0));
    return { p, q, risk: q.slice(CTL_ODD).reduce((a, b) => a + b, 0) };
  }

  // bars: built once; rows below the cut slide down to make room for the cut line
  const ctlRows = CTL_WORDS.map(([w], i) => {
    const r = el('div', 'ctl-row' + (i >= CTL_ODD ? ' is-odd' : ''));
    r.setAttribute('role', 'listitem');
    r.style.setProperty('--i', i);
    const track = el('span', 'ctl-track');
    track.append(el('span', 'ctl-fill'));
    r.append(el('span', 'ctl-word', w), track, el('span', 'ctl-val'));
    return r;
  });
  const ctlCut = el('div', 'ctl-cut');
  ctlCut.setAttribute('aria-hidden', 'true');
  ctlCut.append(el('span', 'ctl-cut-pill'), el('span', 'ctl-cut-line'));
  ctlBars.append(...ctlRows, ctlCut);
  const ctlTicks = CTL_WORDS.map((_, i) => {
    const s = el('span', null, ctlKLabel(i + 1));
    s.style.setProperty('--t', i / (CTL_WORDS.length - 1));
    return s;
  });
  $('#ctlTicks').append(...ctlTicks);

  // gauge: three bands, tick labels outside the arc, the reading inside the opening below the needle
  const G = { cx: 150, cy: 110, r: 92, w: 16 };
  const gAngle = risk => CTL_SWEEP * Math.sqrt(clamp(risk / CTL_MAX));
  const gPoint = (deg, rad) => { const th = (200 - deg) * Math.PI / 180; return [G.cx + rad * Math.cos(th), G.cy - rad * Math.sin(th)]; };
  const gArc = (a, b) => {
    const [x1, y1] = gPoint(a, G.r), [x2, y2] = gPoint(b, G.r);
    return `M${x1.toFixed(2)} ${y1.toFixed(2)} A${G.r} ${G.r} 0 ${b - a > 180 ? 1 : 0} 1 ${x2.toFixed(2)} ${y2.toFixed(2)}`;
  };
  (() => {
    const b1 = gAngle(CTL_BANDS[0]), b2 = gAngle(CTL_BANDS[1]), gap = 1.6;
    let s = `<path class="ctl-band low" d="${gArc(0, b1 - gap / 2)}"/><path class="ctl-band mid" d="${gArc(b1 + gap / 2, b2 - gap / 2)}"/><path class="ctl-band high" d="${gArc(b2 + gap / 2, CTL_SWEEP)}"/>`;
    for (const [v, lab] of [[0, '0%'], [CTL_BANDS[0], '1%'], [CTL_BANDS[1], '5%'], [CTL_MAX, '15%']]) {
      const [x, y] = gPoint(gAngle(v), G.r + G.w / 2 + 14);
      const anchor = x < G.cx - 8 ? 'end' : x > G.cx + 8 ? 'start' : 'middle';
      s += `<text class="ctl-tick" x="${x.toFixed(1)}" y="${(y + 4.5).toFixed(1)}" text-anchor="${anchor}">${lab}</text>`;
    }
    const [nx, ny] = gPoint(0, 72);
    s += `<g class="ctl-needle" id="ctlNeedle" style="transform-origin:${G.cx}px ${G.cy}px"><line x1="${G.cx}" y1="${G.cy}" x2="${nx.toFixed(2)}" y2="${ny.toFixed(2)}"/><circle cx="${G.cx}" cy="${G.cy}" r="9"/></g>`;
    s += `<circle class="ctl-pivot-dot" cx="${G.cx}" cy="${G.cy}" r="3.5"/>`;
    s += `<text class="ctl-num" id="ctlNum" x="${G.cx}" y="${G.cy + 47}" text-anchor="middle">0%</text>`;
    s += `<text class="ctl-band-word" id="ctlBandWord" x="${G.cx}" y="${G.cy + 67}" text-anchor="middle">Low risk</text>`;
    ctlGauge.innerHTML = s;
  })();
  const ctlNeedle = $('#ctlNeedle'), ctlNum = $('#ctlNum'), ctlBandWord = $('#ctlBandWord');

  function ctlTakeaway(T, K, risk) {
    if (K === 1) return 'Always the same word. Right for facts, numbers and code.';
    if (T < 0.4) return 'Almost always the same word. Right for facts, numbers and code.';
    if (T <= 1.1) return risk ? 'Mostly sensible, but a nonsense word can now slip through.' : 'Mostly sensible, with some variety. The usual setting for chat.';
    return risk ? 'Odd words creep in. Fine for brainstorming, risky for anything factual.' : 'Varied, yet top-K keeps every pick sensible. Good for drafting ideas.';
  }

  function ctlRender(T, K) {
    ctl.T = T; ctl.K = K;
    const { p, q, risk } = ctlOdds(T, K);
    const kept = ctlWhole(q.slice(0, K));
    ctlT.value = T; ctlK.value = K;
    paintRange(ctlT); paintRange(ctlK);
    ctlK.setAttribute('aria-valuetext', K >= CTL_WORDS.length ? 'all words' : `top ${K}`);
    $('#ctlTOut').textContent = T.toFixed(1);
    $('#ctlKOut').textContent = ctlKLabel(K);
    ctlTicks.forEach((s, i) => s.classList.toggle('is-on', i + 1 === K));
    ctlRows.forEach((r, i) => {
      const cut = i >= K, x = cut ? p[i] : q[i];
      r.classList.toggle('is-cut', cut);
      r.classList.toggle('is-top', i === 0);
      r.style.setProperty('--cut', cut ? 1 : 0);
      r.children[1].firstChild.style.setProperty('--w', x.toFixed(4));
      r.children[2].textContent = cut ? ctlPct(x) : kept[i];
      r.setAttribute('aria-label', `${CTL_WORDS[i][0]}: ${cut ? 'cut by top-K' : kept[i]}${i >= CTL_ODD ? ', nonsense' : ''}`);
    });
    const all = K >= CTL_WORDS.length;
    // the cut line fades out, jumps and fades back in once the rows have glided apart, so it never crosses a word
    if (K !== ctlCutK) {
      if (ctlCutK) {
        ctlCut.classList.add('is-hidden');
        clearTimeout(ctlCutTimer);
        ctlCutTimer = setTimeout(() => ctlCut.classList.remove('is-hidden'), reduced ? 0 : 380);
      }
      ctlCutK = K;
      ctlCut.style.setProperty('--k', K);
    }
    ctlCut.classList.toggle('is-off', all);
    ctlCut.firstChild.textContent = all ? 'top-K off · all 8 in play' : K === 1 ? 'top 1 kept · the rest cut' : `top ${K} kept · rescaled to 100%`;
    // gauge
    const band = risk === 0 ? 'none' : risk < CTL_BANDS[0] ? 'low' : risk < CTL_BANDS[1] ? 'mid' : 'high';
    const shown = risk === 0 ? '0%' : risk < 0.001 ? '<0.1%' : (risk * 100).toFixed(1) + '%';
    ctlGauge.dataset.band = band;
    ctlNeedle.style.transform = `rotate(${gAngle(risk).toFixed(2)}deg)`;
    ctlNum.textContent = shown;
    ctlBandWord.textContent = band === 'none' ? 'cut by top-K' : band === 'mid' ? 'medium risk' : band + ' risk';
    ctlGauge.setAttribute('aria-label', `Hallucination risk ${shown}: ${ctlBandWord.textContent}`);
    const take = $('#ctlTake');
    take.textContent = ctlTakeaway(T, K, risk);
    take.className = 'ctl-take' + (band === 'mid' || band === 'high' ? ' ' + band : '');
    ctlPresetBtns.forEach(b => {
      const [pt, pk] = CTL_PRESETS[b.dataset.preset];
      const on = Math.abs(pt - T) < 0.05 && pk === K;
      b.classList.toggle('is-on', on);
      b.setAttribute('aria-pressed', String(on));
    });
    ctlChips.classList.toggle('is-stale', !!ctlDrawnAt && ctlDrawnAt !== ctlKey(T, K));
    if (ctlDrawnAt && ctlDrawnAt !== ctlKey(T, K)) $('#ctlTally').textContent = 'Dials moved: generate again';
  }

  function ctlEmpty() {
    ctlDrawnAt = '';
    ctlChips.classList.remove('is-stale');
    ctlChips.replaceChildren(...Array.from({ length: 12 }, () => el('li', 'ctl-chip is-empty', '·')));
    $('#ctlTally').textContent = '12 draws at the current settings';
    const blank = $('#ctlBlank');
    blank.className = 'ctl-blank';
    blank.textContent = '';
  }
  function ctlSample() {
    const T = Math.round(ctl.T * 10) / 10, K = ctl.K;
    const { q } = ctlOdds(T, K);
    const picks = Array.from({ length: 12 }, () => {
      let r = Math.random(), i = 0;
      while (i < K - 1 && (r -= q[i]) > 0) i++;   // never lands on a word top-K has cut
      return i;
    });
    ctlDrawnAt = ctlKey(T, K);
    ctlChips.classList.remove('is-stale');
    ctlChips.replaceChildren(...picks.map((i, n) => {
      const c = el('li', 'ctl-chip' + (i >= CTL_ODD ? ' is-odd' : ''), CTL_WORDS[i][0]);
      c.style.setProperty('--n', n);
      return c;
    }));
    const odd = picks.filter(i => i >= CTL_ODD).length;
    const tally = $('#ctlTally');
    tally.replaceChildren(el('b', odd ? 'is-odd' : null, odd ? `${odd} nonsense` : 'No nonsense'), ` in 12 · T ${T.toFixed(1)}, top-K ${ctlKLabel(K)}`);
    const blank = $('#ctlBlank');
    blank.className = 'ctl-blank';
    void blank.offsetWidth;
    blank.className = 'ctl-blank is-word' + (picks[0] >= CTL_ODD ? ' is-odd' : '');
    blank.textContent = CTL_WORDS[picks[0]][0];
  }

  // smooth moves between settings (presets and the demo); a new move or any touch cancels the one running
  const ctlEase = k => (k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2);
  const ctlStepAny = on => { ctlT.step = on ? 'any' : '0.1'; };
  async function ctlGlide(to, ms, guard = () => true) {
    const me = ++ctlMove, from = [ctl.T, ctl.K], t0 = performance.now();
    if (reduced || ms <= 0) { ctlRender(...to); return true; }
    ctlStepAny(true);
    for (;;) {
      if (me !== ctlMove || !guard()) { if (me === ctlMove) ctlStepAny(false); return false; }
      const k = clamp((performance.now() - t0) / ms), e = ctlEase(k);
      if (k >= 1) { ctlStepAny(false); ctlRender(...to); return true; }
      ctlRender(from[0] + (to[0] - from[0]) * e, Math.round(from[1] + (to[1] - from[1]) * e));
      await new Promise(requestAnimationFrame);
    }
  }
  function ctlStop() {
    ctlTouched = true;
    ctlDemo++; ctlMove++;
    ctlStepAny(false);
    ctl.T = Math.round(ctl.T * 10) / 10;
  }
  async function ctlPlay() {
    const me = ++ctlDemo;
    const live = () => me === ctlDemo && !ctlTouched && isActive('controls');
    const wait = async ms => { await sleep(ms); return live(); };
    ctlMove++;
    ctlRender(...CTL_PRESETS.facts);
    ctlEmpty();
    if (!await wait(1300)) return;
    ctlSample();
    if (!await wait(1500)) return;
    if (!await ctlGlide(CTL_PRESETS.brainstorm, 2000, live)) return;
    if (!await wait(350)) return;
    ctlSample();
    if (!await wait(2600)) return;
    if (!await ctlGlide(CTL_PRESETS.chat, 1400, live)) return;
    if (!await wait(350)) return;
    ctlSample();
  }

  ctlT.addEventListener('input', () => { ctlStop(); ctlRender(Math.round(+ctlT.value * 10) / 10, ctl.K); });
  ctlK.addEventListener('input', () => { ctlStop(); ctlRender(ctl.T, +ctlK.value); });
  ctlPresetBtns.forEach(b => b.addEventListener('click', () => { ctlStop(); ctlGlide(CTL_PRESETS[b.dataset.preset], 650); }));
  $('#ctlSample').addEventListener('click', () => { ctlStop(); ctlSample(); });
  // the presenter takes over the moment they touch the panel
  $('#ctlPanel').addEventListener('pointerdown', () => { if (!ctlTouched) ctlStop(); });
  $('#ctlPanel').addEventListener('keydown', () => { if (!ctlTouched) ctlStop(); });

  ctlRender(...CTL_PRESETS.chat);
  ctlSample();
  hooks.controls = () => {
    if (reduced || ctlTouched) { ctlRender(ctl.T, ctl.K); return; }
    ctlPlay();
  };
  /* BUILD:controls END */

  /* BUILD:memory START */
  /* ------------------------------------------------------------------ 7: memory: without and with */
  // Two chat apps get the same messages from Priya. The stateless one sends the model only the newest message; the
  // one with memory sends the whole thread every turn and saves it under its thread_id after every step. "Come back
  // tomorrow" closes both apps: only the saved thread comes back. Token counts are illustrative: the deck's
  // tokenizer, plus 4 tokens of chat formatting per message, plus a short system prompt.
  const MEM_TURNS = [
    { q: 'I’m Priya from Finance. Summarise last month’s travel report.',
      a: ['Travel spend was 18% over budget, mainly because of two unplanned client visits and higher airfares.',
        'Travel spend was 18% over budget, mainly because of two unplanned client visits and higher airfares.'],
      ref: [null, null] },
    { q: 'And without those two visits?',
      a: ['Which visits do you mean? I can’t see any earlier messages.', 'Without those two visits, travel was 3% under budget.'],
      ref: ['no history', 'used: turn 1'] },
    { q: 'Draft a two-line note to my team about it.',
      a: ['Happy to help. Who is your team, and what should the note say?',
        'Hi team: travel was 18% over budget last month because of two unplanned client visits. Without them, we were 3% under. Thanks,\u00a0Priya'],
      ref: ['no history', 'used: turns 1, 2'] },
    { q: 'Morning! Can you shorten that note to one line?',
      a: ['Which note do you mean? This looks like a new conversation.', 'Hi team: two unplanned client visits put travel 18% over budget last month.\u00a0Priya'],
      ref: ['no history', 'used: saved thread'] },
  ];
  const MEM_SYS = 28;
  const memTok = t => tokenize(t).length + 4;
  const memSide = [0, 1].map(k => ({
    chat: $('#memChat' + k), chips: $('#memChips' + k), count: $('#memCount' + k), cols: $$('#memCols' + k + ' i'),
    cover: $('#memCover' + k), badge: $('#memBadge' + k), shown: 0,
  }));
  const memBtn = { next: $('#memNext'), tomorrow: $('#memTomorrow'), replay: $('#memReplay') };
  const memWho = $('#memWho'), memStepEl = $('#memStep');
  let memRun = 0, memAt = 0, memBusy = false, memAuto = true;
  const memOk = me => me === memRun && isActive('memory');
  const memRestart = (n, cls) => { n.classList.remove(cls); void n.offsetWidth; n.classList.add(cls); };

  // what the model receives at turn n (1–4): the stateless app sends the system prompt and the newest message only
  function memSent(s, n) {
    const you = k => ({ label: 'Priya ' + k, cls: 'you', tok: memTok(MEM_TURNS[k - 1].q) });
    const ai = k => ({ label: 'AI ' + k, cls: 'ai', tok: memTok(MEM_TURNS[k - 1].a[s]) });
    const list = [{ label: 'System', cls: 'sys', tok: MEM_SYS }];
    if (n === 4) {
      if (s) list.push({ label: 'Yesterday’s thread · 6', cls: 'thread', tok: [1, 2, 3].reduce((t, k) => t + you(k).tok + ai(k).tok, 0) });
    } else {
      for (let k = 1; k < n; k++) list.push(...[you(k), ai(k)].map(c => (s ? c : { ...c, cls: 'ghost', tok: 0 })));
    }
    list.push(you(n));
    return list;
  }
  const memTotal = (s, n) => memSent(s, n).reduce((t, c) => t + c.tok, 0);
  const memShow = v => (v >= 1 ? '~' + Math.max(5, Math.round(v / 5) * 5) : '0');
  const MEM_MAX = Math.max(...[1, 2, 3, 4].map(n => memTotal(1, n)));

  function memChip(c, animate) {
    const n = el('span', 'mem-chip ' + c.cls, c.label);
    if (c.cls === 'ghost') { n.title = 'Shown on screen, not sent to the model'; n.setAttribute('aria-label', c.label + ', not sent'); }
    if (!animate) n.style.animation = 'none';
    return n;
  }
  function memCountTo(side, to, animate) {
    const from = side.shown;
    side.shown = to;
    if (!animate || reduced) { side.count.textContent = memShow(to); return; }
    const me = memRun, t0 = performance.now();
    const tick = now => {
      if (me !== memRun) return;
      const k = clamp((now - t0) / 320);
      side.count.textContent = memShow(from + (to - from) * (1 - Math.pow(1 - k, 3)));
      if (k < 1) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }
  function memCols(s, n) {
    memSide[s].cols.forEach((c, k) => {
      c.style.setProperty('--h', k < n ? (memTotal(s, k + 1) / MEM_MAX).toFixed(3) : 0);
      c.classList.toggle('now', k === n - 1);
    });
  }

  // chat pieces
  const memStill = n => { n.style.animation = 'none'; return n; };
  function memRef(a, s, n, animate) {
    const r = MEM_TURNS[n - 1].ref[s];
    if (!r) return;
    const c = el('span', 'mem-ref' + (s ? '' : ' no'), r);
    a.append(' ', animate ? c : memStill(c));   // a real space, so a wrapped chip starts flush left
  }
  function memA(s, n, full) {
    const a = el('p', 'mem-msg mem-a' + (!s && n > 1 ? ' lost' : ''));
    if (full) { a.append(el('span', 'mem-text', MEM_TURNS[n - 1].a[s])); memRef(a, s, n, false); }
    else a.innerHTML = '<span class="mem-dots" role="img" aria-label="Writing"><i></i><i></i><i></i></span>';
    return a;
  }
  function memTurnEl(s, n, full, animate) {
    const g = el('div', 'mem-turn');
    g.dataset.t = n;
    g.append(el('p', 'mem-msg mem-q' + (animate ? ' mem-in' : ''), MEM_TURNS[n - 1].q));
    if (full) g.append(memA(s, n, true));
    return g;
  }
  const memLine = (cls, text, animate) => { const n = el('p', cls, text); return animate ? n : memStill(n); };
  const memReceipt = (s, animate, text) => memLine('mem-receipt', text || (s ? '✓ saved to the thread' : 'nothing saved'), animate);
  function memFoldEl(animate) {
    const w = el('div', 'mem-foldwrap');
    w.append(memStill(el('p', 'mem-fold', 'Yesterday · 4 earlier messages')));
    if (!animate) memStill(w);
    return w;
  }
  function memCover(side, kind, text, id) {
    side.cover.className = 'mem-cover' + (kind === 'loading' ? ' loading' : '');
    $('p', side.cover).textContent = text;
    if (id) $('p', side.cover).append(el('span', 'mem-cover-id', id));
    side.cover.hidden = false;
  }

  function memControls() {
    memStepEl.textContent = memAt === 4 ? 'Next morning' : `Turn ${Math.max(1, memAt)} of 3`;
    memBtn.next.disabled = memAt >= 3;
    memBtn.tomorrow.disabled = memAt !== 3;
    memBtn.tomorrow.classList.toggle('invite', memAt === 3 && !memBusy);
  }

  // the finished state after turn n (0–3, or 4 for the next morning), drawn without motion
  function memRender(n) {
    memRun++;
    memAt = n;
    memBusy = false;
    memSide.forEach((side, s) => {
      side.cover.hidden = true;
      side.chat.classList.remove('mem-off');
      side.chat.removeAttribute('aria-busy');
      const kids = [];
      if (n === 4) {
        if (s) kids.push(memFoldEl(false), memTurnEl(s, 3, true), memLine('mem-day', 'Next morning · same thread'));
        else kids.push(memLine('mem-day', 'Next morning · new conversation'));
        kids.push(memTurnEl(s, 4, true));
      } else {
        for (let k = 1; k <= n; k++) kids.push(memTurnEl(s, k, true));
      }
      if (n) kids.push(memReceipt(s, false));
      side.chat.replaceChildren(...kids);
      side.chips.replaceChildren(...(n ? memSent(s, n).map(c => memChip(c, false)) : []));
      memCountTo(side, n ? memTotal(s, n) : 0, false);
      memCols(s, n);
    });
    memControls();
  }

  // the harness builds this turn's request: each message it sends lights up as its chip joins the stack
  async function memSend(s, n, me) {
    const side = memSide[s];
    const list = memSent(s, n);
    const msgs = $$('.mem-msg, .mem-fold', side.chat);
    side.chips.replaceChildren();
    memCountTo(side, 0, false);
    let k = 0, sum = 0;
    for (const c of list) {
      if (me !== memRun) return;
      side.chips.append(memChip(c, true));
      const lit = c.cls === 'thread' ? msgs.slice(0, -1) : c === list[list.length - 1] ? [msgs[msgs.length - 1]] : c.cls === 'ghost' || c.cls === 'sys' ? [] : [msgs[k++]];
      lit.forEach(m => m && memRestart(m, 'mem-lit'));
      sum += c.tok;
      memCountTo(side, sum, true);
      await sleep(c.cls === 'ghost' ? 90 : 170);
    }
    memCols(s, n);
  }
  async function memType(a, s, n, me) {
    const span = el('span', 'mem-text');
    a.replaceChildren(span);
    for (const t of tokenize(MEM_TURNS[n - 1].a[s])) {
      if (me !== memRun) return;
      span.textContent += t;
      await sleep(34);
    }
    memRef(a, s, n, true);
  }

  // overnight: both apps close; only the one with a thread_id gets its conversation back
  async function memMorning(me) {
    const [s0, s1] = memSide;
    memSide.forEach(side => { side.chat.classList.add('mem-off'); memCover(side, 'night', 'Priya closes the app for the night'); });
    await sleep(1500); if (!memOk(me)) return null;
    s0.chat.replaceChildren(memLine('mem-day', 'Next morning · new conversation', true));
    s0.chips.replaceChildren();
    memCountTo(s0, 0, true);
    s0.chat.classList.remove('mem-off');
    s0.cover.hidden = true;
    memCover(s1, 'loading', 'Loading the saved thread', 'thread_id: finance-priya-0412');
    await sleep(1400); if (!memOk(me)) return null;
    s1.chat.replaceChildren(...[1, 2, 3].map(k => memTurnEl(1, k, true)), memReceipt(1, true, '✓ thread restored from our database'));
    $$('.mem-msg', s1.chat).forEach((m, k) => { m.classList.add('mem-restore'); m.style.setProperty('--k', k); });
    s1.chat.classList.remove('mem-off');
    s1.cover.hidden = true;
    memRestart(s1.badge, 'mem-save');
    await sleep(2300); if (!memOk(me)) return null;
    // Priya carries on: yesterday's first two turns fold away to make room, but they are still sent
    s1.chat.querySelector('.mem-receipt')?.remove();
    const old = $$('.mem-turn', s1.chat).filter(g => +g.dataset.t < 3);
    const wrap = memFoldEl(true);
    wrap.classList.add('mem-going');
    wrap.style.height = '0px';
    wrap.style.opacity = '0';
    s1.chat.prepend(wrap);
    old.forEach(g => { g.style.height = g.offsetHeight + 'px'; g.classList.add('mem-going'); });
    void s1.chat.offsetHeight;
    wrap.style.height = wrap.firstChild.offsetHeight + 'px';
    wrap.style.opacity = '1';
    old.forEach(g => { g.style.height = '0px'; g.style.opacity = '0'; g.style.marginTop = '0px'; });
    await sleep(600); if (!memOk(me)) return null;
    old.forEach(g => g.remove());
    wrap.classList.remove('mem-going');
    wrap.style.height = '';
    s1.chat.append(memLine('mem-day', 'Next morning · same thread', true));
    await sleep(350); if (!memOk(me)) return null;
    return memSide.map((side, s) => { const g = memTurnEl(s, 4, false, true); side.chat.append(g); return g; });
  }

  // one turn, animated from the finished state of the turn before
  async function memPlay(n) {
    const me = ++memRun;
    memAt = n;
    memBusy = true;
    memControls();
    memSide.forEach(side => side.chat.querySelector('.mem-receipt')?.remove());
    const groups = n === 4 ? await memMorning(me) : memSide.map((side, s) => { const g = memTurnEl(s, n, false, true); side.chat.append(g); return g; });
    if (!groups || !memOk(me)) return;
    memRestart(memWho, 'send');
    await sleep(500); if (!memOk(me)) return;
    await Promise.all([0, 1].map(s => memSend(s, n, me)));
    await sleep(300); if (!memOk(me)) return;
    const as = groups.map((g, s) => { const a = memA(s, n, false); a.classList.add('mem-in'); g.append(a); return a; });
    await sleep(700); if (!memOk(me)) return;
    memSide.forEach(side => side.chat.setAttribute('aria-busy', 'true'));
    await Promise.all(as.map((a, s) => memType(a, s, n, me)));
    memSide.forEach(side => side.chat.removeAttribute('aria-busy'));
    if (!memOk(me)) return;
    await sleep(250); if (!memOk(me)) return;
    memSide.forEach((side, s) => side.chat.append(memReceipt(s, true)));
    memRestart(memSide[1].badge, 'mem-save');
    memBusy = false;
    memControls();
    if (memAuto && n < 3) {
      await sleep(3800);
      if (memOk(me) && memAuto) memPlay(n + 1);
    }
  }
  async function memStart(arriving) {
    memRender(0);
    if (reduced) { memRender(arriving ? 3 : 1); return; }
    const me = memRun;
    await sleep(arriving ? 900 : 300);
    if (memOk(me)) memPlay(1);
  }
  memBtn.next.addEventListener('click', () => {
    memAuto = false;
    if (memAt >= 3) return;
    const n = memAt;
    if (memBusy) memRender(n);           // finish the turn on screen first
    if (reduced) memRender(n + 1); else memPlay(n + 1);
  });
  memBtn.tomorrow.addEventListener('click', () => {
    memAuto = false;
    if (memAt !== 3) return;
    if (memBusy) memRender(3);
    if (reduced) memRender(4); else memPlay(4);
  });
  memBtn.replay.addEventListener('click', () => { memAuto = true; memStart(false); });
  memRender(3);
  hooks.memory = () => { memAuto = true; memStart(true); };
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
  // On arrival the HR policy is cut into chunks, each chunk becomes numbers and flies into the map as a dot; then the
  // first question runs the path back: its numbers drop a star onto the map, lines reach the K nearest chunks the
  // asker may see, those chunks lift into the prompt and the answer types with its source. Chips, top-K and the
  // "change §4.1" button replay the relevant part. Every sequence carries a run number and stops when a newer one
  // starts or the slide leaves the screen (it then jumps to its finished state).
  {
    const NS = 'http://www.w3.org/2000/svg';
    const ragBody = $('#ragBody'), ragFly = $('#ragFly'), ragSvg = $('#ragSvg');
    const inRail = $('#ragInRail'), askRail = $('#ragAskRail');
    const LOCK = '<svg class="rag-lock" viewBox="0 0 24 24" aria-hidden="true"><rect x="5" y="10.5" width="14" height="10" rx="2.5"/><path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5"/></svg>';
    const sgn = v => (v < 0 ? '−' : '') + Math.abs(v).toFixed(2);
    const rng = seed => () => ((seed = (seed * 16807) % 2147483647) / 2147483647);
    const WEAK = 0.4;   // below this similarity a retrieved chunk only dilutes the prompt
    // one-line texts are cut at a word, never by CSS, so no hidden text runs on under its neighbours
    function fit(node, text, quote = false) {
      node.dataset.full = text;
      node.dataset.quote = quote ? '1' : '';
      const wrap = t => (quote ? `“${t}”` : t);
      node.textContent = wrap(text);
      if (!node.clientWidth || node.scrollWidth <= node.clientWidth + 0.5) return;
      const words = text.split(' ');
      while (words.length > 2) {
        words.pop();
        node.textContent = wrap(words.join(' ').replace(/[,;:.]$/, '') + '…');
        if (node.scrollWidth <= node.clientWidth + 0.5) return;
      }
    }
    const refit = () => $$('#rag [data-full]').forEach(n => fit(n, n.dataset.full, !!n.dataset.quote));

    // The illustrative HR policy, one chunk per section. x, y place each dot on the 480 × 400 map and `side` its
    // label; positions, label sides and the question stars were solved together so no label touches a line.
    const CH = [
      { sec: '§4.1', name: 'Annual leave', tile: '§4.1 Annual', h: 1, x: 164, y: 109, side: 'T',
        text: d => `Full-time staff get ${d} days of paid annual leave a year, plus public holidays.` },
      { sec: '§4.2', name: 'Carry-over', tile: '§4.2 Carry-over', h: 2, x: 274, y: 94, side: 'T',
        text: () => 'Up to 5 unused days can be carried into the next year; they expire on 31 March.' },
      { sec: '§4.3', name: 'Parental leave', tile: '§4.3 Parental', h: 4, x: 226, y: 298, side: 'B',
        text: () => '26 weeks paid for the primary carer and 4 weeks for the secondary carer, after six months of service.' },
      { sec: '§4.4', name: 'Sick leave', tile: '§4.4 Sick', h: 3, x: 116, y: 257, side: 'L',
        text: () => 'Up to 12 paid days a year; a doctor’s note after 3 days in a row.' },
      { sec: '§5.1', name: 'Remote work', tile: '§5.1 Remote', h: 5, x: 377, y: 146, side: 'T',
        text: () => 'Up to 2 days a week with the manager’s agreement.' },
      { sec: 'HR only', name: 'Payroll bands', tile: 'Payroll bands', h: 0, x: 400, y: 320, side: 'B', hr: true,
        text: () => 'Salary bands for every grade. Tagged HR only: never returned outside HR.' },
    ];
    CH.forEach((c, i) => { const r = rng(4099 + i * 7919); c.vec = Array.from({ length: 24 }, () => Math.round((r() * 2 - 1) * 100) / 100); });
    CH[0].vec.splice(0, 4, 0.21, -0.07, 0.88, 0.13);
    const v41b = CH[0].vec.map((v, i) => (i === 0 ? 0.22 : i === 2 ? 0.87 : i % 5 === 3 ? clamp(v + 0.06, -1, 1) : v));   // §4.1 after the edit

    // hits: [chunk, similarity] nearest first among chunks the asker may see; blocked: nearest overall, filtered out
    const QS = [
      { q: 'How many days of annual leave do I get?', star: [177, 152], hits: [[0, 0.89], [1, 0.71], [3, 0.43], [2, 0.38]],
        ans: (k, d) => [`You get ${d} days of paid annual leave a year, plus public holidays.`, [0]] },
      { q: 'Can I carry unused leave into next year?', star: [263, 140], hits: [[1, 0.91], [0, 0.74], [4, 0.31], [2, 0.27]],
        ans: (k, d) => (k > 1 ? [`Yes: up to 5 of your ${d} days can be carried into next year; they expire on 31 March.`, [1, 0]]
          : ['Yes: up to 5 unused days can be carried into next year; they expire on 31 March.', [1]]) },
      { q: 'What does parental leave look like?', star: [218, 254], hits: [[2, 0.88], [3, 0.49], [0, 0.44], [1, 0.35]],
        ans: () => ['Primary carers get 26 weeks paid and secondary carers 4 weeks, after six months of service.', [2]] },
      { q: 'What are the salary bands?', star: [359, 252], blocked: [5, 0.86], hits: [[4, 0.24], [2, 0.21], [1, 0.15], [0, 0.12]],
        ans: () => ['I can’t share salary bands: they aren’t in any document you may see. Please ask HR.', []] },
    ];

    let run = 0, qi = 0, K = 3, days = 24, ingested = false, starFor = -1;
    const vecOf = i => (i === 0 && days === 25 ? v41b : CH[i].vec);
    const qVec = q => {
      const Q = QS[q], r = rng(77 + q * 131);
      const a = vecOf(Q.blocked ? Q.blocked[0] : Q.hits[0][0]), b = vecOf(Q.hits[Q.blocked ? 0 : 1][0]);
      return a.map((v, i) => clamp(v * 0.82 + b[i] * 0.12 + (r() - 0.5) * 0.16, -1, 1));
    };

    /* ---------- building blocks */
    const tilesBox = $('#ragTiles');
    const tiles = CH.map((c, i) => {
      const li = el('li', `rag-tile rag-h${c.h}` + (c.hr ? ' is-lock' : ''));
      const b = el('button', null, c.tile);
      b.type = 'button';
      if (c.hr) b.insertAdjacentHTML('beforeend', LOCK);
      b.setAttribute('aria-label', `${c.name}${c.hr ? ', tagged HR only' : ''}: show this chunk as numbers`);
      b.addEventListener('click', () => showEmbed(i, true, true, true));
      li.append(b);
      return li;
    });
    tilesBox.replaceChildren(...tiles);

    const mkCells = box => { box.replaceChildren(...Array.from({ length: 24 }, () => el('i'))); return [...box.children]; };
    const cellsIn = mkCells($('#ragCellsIn')), cellsQ = mkCells($('#ragCellsQ'));
    const paintCells = (cells, vec, upto = 24) => cells.forEach((c, i) => c.style.setProperty('--o', vec && i < upto ? (0.1 + 0.9 * (vec[i] + 1) / 2).toFixed(2) : 0.08));

    const svgEl = (tag, attrs = {}, cls) => {
      const n = document.createElementNS(NS, tag);
      for (const k in attrs) n.setAttribute(k, attrs[k]);
      if (cls) n.setAttribute('class', cls);
      return n;
    };
    const gLinks = svgEl('g'), gPts = svgEl('g'), gStar = svgEl('g');
    ragSvg.append(gLinks, gPts, gStar);
    const pts = CH.map(c => {
      const g = svgEl('g', { transform: `translate(${c.x} ${c.y})` }, `rag-pt rag-h${c.h} is-off` + (c.hr ? ' rag-pt-lock' : ''));
      const pop = svgEl('g', {}, 'rag-pop');
      pop.append(svgEl('circle', { r: c.hr ? 11 : 9 }, 'rag-dot'));
      if (c.hr) { const lk = svgEl('g', { transform: 'translate(-6 -6.6) scale(0.5)' }, 'rag-lockg'); lk.innerHTML = '<rect x="5" y="10.5" width="14" height="10" rx="2.5"/><path d="M8.5 10.5V8a3.5 3.5 0 0 1 7 0v2.5"/>'; pop.append(lk); }
      // a 38-unit label block beside the dot: the name, then the section (and, when retrieved, the similarity)
      const [ax, anchor, y0] = c.side === 'L' ? [-16, 'end', -19] : c.side === 'R' ? [16, 'start', -19] : c.side === 'T' ? [0, 'middle', -51] : [0, 'middle', 18];
      const n = svgEl('text', { x: ax, y: y0 + 14, 'text-anchor': anchor }, 'rag-lab-n');
      n.textContent = c.name;
      const s = svgEl('text', { x: ax, y: y0 + 31, 'text-anchor': anchor }, 'rag-lab-s');
      s.textContent = c.sec;
      g.append(pop, n, s);
      gPts.append(g);
      return { g, pop, s };
    });
    const STAR = (() => { let d = ''; for (let k = 0; k < 10; k++) { const r = k % 2 ? 5.4 : 13, a = -Math.PI / 2 + k * Math.PI / 5; d += (k ? 'L' : 'M') + (r * Math.cos(a)).toFixed(2) + ' ' + (r * Math.sin(a)).toFixed(2); } return d + 'Z'; })();
    const starG = svgEl('g'), starPop = svgEl('g', {}, 'rag-pop');
    starPop.append(svgEl('path', { d: STAR }, 'rag-star'));
    starG.append(starPop);
    starG.style.display = 'none';
    gStar.append(starG);
    // a key in the empty bottom-left corner, so a photo of the map still reads
    const key = svgEl('g', { transform: 'translate(22 381)' }, 'rag-key');
    key.innerHTML = `<path class="rag-star" transform="scale(0.62)" d="${STAR}"/><text class="rag-lab-s" x="14" y="4.5">question</text>`
      + '<circle class="rag-dot rag-key-dot" cx="98" cy="0" r="6"/><text class="rag-lab-s" x="110" y="4.5">chunk</text>'
      + '<line class="rag-link" x1="164" y1="0" x2="186" y2="0" stroke-width="3"/><text class="rag-lab-s" x="194" y="4.5">nearest K</text>';
    ragSvg.insertBefore(key, gLinks);

    /* ---------- the two lanes */
    const railSt = rail => $$('.rag-st', rail);
    function sizeRails() { [inRail, askRail].forEach(r => { const st = railSt(r); r.style.setProperty('--end', st[st.length - 1].offsetTop + 'px'); }); }
    function stage(rail, key) {   // key = the station now working; null = the lane has finished
      const st = railSt(rail);
      const idx = key == null ? st.length : st.findIndex(s => s.dataset.st === key);
      st.forEach((s, j) => { s.classList.toggle('is-on', j === idx); s.classList.toggle('is-done', j < idx); });
      rail.style.setProperty('--prog', st[Math.min(idx, st.length - 1)].offsetTop + 'px');
      rail.classList.toggle('is-run', idx < st.length);
    }
    function resetRail(rail) { railSt(rail).forEach(s => s.classList.remove('is-on', 'is-done')); rail.style.setProperty('--prog', '0px'); rail.classList.remove('is-run'); }

    /* ---------- motion helpers (canvas px, relative to .rag-body) */
    const relBox = n => { const b = ragBody.getBoundingClientRect(), r = n.getBoundingClientRect(); return { x: (r.left - b.left) / S, y: (r.top - b.top) / S, w: r.width / S, h: r.height / S }; };
    const mid = n => { const r = relBox(n); return { x: r.x + r.w / 2, y: r.y + r.h / 2 }; };
    const endOf = (n, right) => { const r = relBox(n); return { x: right ? r.x + r.w - 4 : r.x + 4, y: r.y + r.h / 2 }; };
    const mapXY = (x, y) => { const b = ragBody.getBoundingClientRect(), r = ragSvg.getBoundingClientRect(), k = r.width / 480; return { x: (r.left - b.left + x * k) / S, y: (r.top - b.top + y * k) / S }; };
    function flyTo(node, a, b, ms, lift = 60, delay = 0) {   // along a gentle arc; resolves true when it lands
      ragFly.append(node);
      const c = { x: (a.x + b.x) / 2, y: Math.min(a.y, b.y) - lift };
      const frames = [];
      for (let k = 0; k <= 16; k++) {
        const t = k / 16, u = 1 - t;
        const x = u * u * a.x + 2 * u * t * c.x + t * t * b.x, y = u * u * a.y + 2 * u * t * c.y + t * t * b.y;
        frames.push({ transform: `translate(${x.toFixed(1)}px, ${y.toFixed(1)}px) scale(${(0.75 + 0.25 * Math.sin(Math.PI * t)).toFixed(2)})`, opacity: t < 0.1 ? t * 10 : 1 });
      }
      const an = node.animate(frames, { duration: ms, delay, easing: 'cubic-bezier(.45,.05,.3,1)', fill: 'both' });
      return an.finished.then(() => { node.remove(); return true; }, () => { node.remove(); return false; });
    }
    const cancelFly = () => { ragFly.getAnimations({ subtree: true }).forEach(a => a.cancel()); ragFly.replaceChildren(); };
    const pop = n => n.animate([{ transform: 'scale(0)' }, { transform: 'scale(1.35)', offset: 0.6 }, { transform: 'scale(1)' }], { duration: 420, easing: 'ease-out' });
    const nudge = n => n.animate([{ transform: 'none' }, { transform: 'scale(1.03)' }, { transform: 'none' }], { duration: 420, easing: 'ease-in-out' });
    function ripple(parent, x, y) {
      const c = svgEl('circle', { cx: x, cy: y, r: 13 }, 'rag-ripple rag-pop');
      parent.prepend(c);
      c.animate([{ transform: 'scale(1)', opacity: 0.6 }, { transform: 'scale(4.5)', opacity: 0 }], { duration: 900, easing: 'ease-out', fill: 'forwards' }).finished.then(() => c.remove(), () => c.remove());
    }
    async function step(me, ms) {   // wait, then say whether this run should carry on
      if (ms) await sleep(ms);
      if (me !== run) return false;
      if (!isActive('rag')) { run++; renderFinal(); return false; }
      return true;
    }

    /* ---------- ingest */
    let embRun = 0;
    async function showEmbed(i, animate, slow = true, keep = false) {   // the chunk as text, then as numbers (keep: leave it marked)
      const me = ++embRun, v = vecOf(i);
      const nums = $('#ragNums');
      fit($('#ragQuote'), CH[i].text(days), true);
      const final = () => { nums.textContent = `[${v.slice(0, 4).map(sgn).join(', ')}, … ]`; paintCells(cellsIn, v); };
      if (animate) tiles.forEach((t, j) => t.classList.toggle('is-cur', j === i));
      if (!animate || reduced) { final(); return; }
      if (!slow) { final(); return; }
      nums.textContent = '[';
      paintCells(cellsIn, null);
      for (let k = 0; k < 4; k++) {
        await sleep(150);
        if (me !== embRun) return;
        nums.append(el('span', 'new', (k ? ', ' : '') + sgn(v[k])));
        paintCells(cellsIn, v, (k + 1) * 6);
      }
      nums.append(', … ]');
      await sleep(700);
      if (me === embRun && !keep) tiles[i].classList.remove('is-cur');
    }
    function setCount(n) { $('#ragCount').textContent = n === 1 ? '1 chunk' : `${n} chunks`; }
    function syncDoc() {
      $('#ragDocMeta').textContent = days === 25 ? 'edited' : '6 sections';
      $('#ragDoc').classList.toggle('is-new', days === 25);
      tiles[0].classList.toggle('is-new', days === 25);
      const u = $('#ragUpd');
      u.textContent = days === 25 ? 'Undo: back to 24 days' : 'Change §4.1 to 25 days';
      u.setAttribute('aria-pressed', String(days === 25));
    }
    function renderIngest() {
      embRun++;
      tilesBox.classList.remove('is-pre');
      tiles.forEach(t => t.classList.remove('is-cur'));
      pts.forEach(p => p.g.classList.remove('is-off'));
      setCount(6);
      syncDoc();
      showEmbed(0, false);
      stage(inRail, null);
      inRail.classList.remove('is-run');
      ingested = true;
    }
    async function playIngest(me) {
      ingested = false;
      embRun++;
      pts.forEach(p => p.g.classList.add('is-off'));
      setCount(0);
      tilesBox.classList.add('is-pre');
      tiles.forEach(t => t.classList.remove('is-cur'));
      $('#ragQuote').textContent = '';
      $('#ragNums').textContent = '';
      paintCells(cellsIn, null);
      resetRail(inRail);
      if (!(await step(me, 500))) return false;
      stage(inRail, 'doc');
      nudge($('#ragDoc'));
      if (!(await step(me, 420))) return false;
      // the document splits: each chunk card grows out of the document card
      stage(inRail, 'chunk');
      const d = mid($('#ragDoc'));
      tilesBox.classList.remove('is-pre');
      tiles.forEach((t, n) => {
        const m = mid(t);
        t.animate([{ transform: `translate(${d.x - m.x}px, ${d.y - m.y}px) scale(0.3)`, opacity: 0 }, { transform: 'none', opacity: 1 }],
          { duration: 560, delay: n * 70, easing: 'cubic-bezier(.2,.7,.2,1)', fill: 'backwards' });
      });
      if (!(await step(me, 720))) return false;
      // each chunk becomes numbers, then flies into the store as a dot
      stage(inRail, 'embed');
      let landed = 0, last = null;
      for (let i = 0; i < CH.length; i++) {
        showEmbed(i, true, i === 0);
        if (!(await step(me, i === 0 ? 850 : 230))) return false;
        const c = CH[i];
        last = flyTo(el('i', `rag-pkt rag-h${c.h}`), endOf($('#ragCellsIn'), true), mapXY(c.x, c.y), 600, 36).then(ok => {
          if (!ok || me !== run) return;
          pts[i].g.classList.remove('is-off');
          pop(pts[i].pop);
          setCount(++landed);
        });
        if (i === 0) stage(inRail, 'store');
      }
      await last;
      if (!(await step(me, 350))) return false;
      tiles.forEach(t => t.classList.remove('is-cur'));
      showEmbed(0, false);
      stage(inRail, null);
      inRail.classList.remove('is-run');
      ingested = true;
      return true;
    }

    /* ---------- ask */
    function clearPts() { pts.forEach((p, i) => { p.g.classList.remove('is-hit', 'is-weak', 'is-x'); p.s.textContent = CH[i].sec; }); }
    function markPts(Q, k) {
      const hit = new Map(Q.hits.slice(0, k));
      pts.forEach((p, i) => {
        const s = hit.get(i), blocked = Q.blocked && Q.blocked[0] === i;
        p.g.classList.toggle('is-hit', s != null && s >= WEAK);
        p.g.classList.toggle('is-weak', s != null && s < WEAK);
        p.g.classList.toggle('is-x', !!blocked);
        p.s.textContent = blocked ? 'HR only · filtered' : s != null ? `${CH[i].sec} · ${s.toFixed(2)}` : CH[i].sec;
      });
    }
    function placeStar(Q, animate) {
      starG.setAttribute('transform', `translate(${Q.star[0]} ${Q.star[1]})`);
      starG.style.display = '';
      starFor = qi;
      if (!animate || reduced) return;
      starPop.animate([{ transform: 'translateY(-30px) scale(0.4)', opacity: 0 }, { transform: 'translateY(2px) scale(1.15)', opacity: 1, offset: 0.7 }, { transform: 'none', opacity: 1 }], { duration: 480, easing: 'ease-out' });
      ripple(starG, 0, 0);
    }
    function drawLinks(Q, k, animate) {
      gLinks.replaceChildren();
      const [sx, sy] = Q.star;
      const list = (Q.blocked ? [{ i: Q.blocked[0], s: Q.blocked[1], x: true }] : []).concat(Q.hits.slice(0, k).map(([i, s]) => ({ i, s })));
      list.forEach((h, n) => {
        const c = CH[h.i], dx = c.x - sx, dy = c.y - sy, d = Math.hypot(dx, dy), ux = dx / d, uy = dy / d;
        const a = [sx + ux * 16, sy + uy * 16], b = [c.x - ux * ((c.hr ? 11 : 9) + 5), c.y - uy * ((c.hr ? 11 : 9) + 5)];
        const len = Math.hypot(b[0] - a[0], b[1] - a[1]);
        const ln = svgEl('line', { x1: a[0].toFixed(1), y1: a[1].toFixed(1), x2: b[0].toFixed(1), y2: b[1].toFixed(1) }, h.x ? 'rag-link-x' : 'rag-link' + (h.s < WEAK ? ' is-weak' : ''));
        gLinks.append(ln);
        if (h.x) {   // the access filter stops the search here: a cross halfway along
          const xm = svgEl('g', { transform: `translate(${((a[0] + b[0]) / 2).toFixed(1)} ${((a[1] + b[1]) / 2).toFixed(1)})` });
          const xin = svgEl('g', {}, 'rag-pop');
          xin.innerHTML = '<circle class="rag-xmark" r="8"/><path class="rag-xmark-l" d="M-3.2 -3.2L3.2 3.2M3.2 -3.2L-3.2 3.2"/>';
          xm.append(xin);
          gLinks.append(xm);
          if (animate && !reduced) { ln.animate([{ opacity: 0 }, { opacity: 1 }], { duration: 300, fill: 'backwards' }); xin.animate([{ transform: 'scale(0)' }, { transform: 'scale(1.3)', offset: 0.6 }, { transform: 'scale(1)' }], { duration: 420, delay: 200, fill: 'backwards' }); }
          return;
        }
        ln.setAttribute('stroke-width', (1.5 + h.s * 3).toFixed(1));
        ln.style.strokeDasharray = len.toFixed(1);
        if (animate && !reduced) ln.animate([{ strokeDashoffset: len }, { strokeDashoffset: 0 }], { duration: 440, delay: (n + (Q.blocked ? 0.6 : 0)) * 130, easing: 'cubic-bezier(.3,.7,.2,1)', fill: 'backwards' });
      });
    }
    function setFind(Q, k) {
      const f = $('#ragFind');
      f.replaceChildren(el('span', null, `top ${k}`), el('span', 'rag-tag', 'access: all staff'));
      if (Q.blocked) f.append(el('span', 'rag-tag is-x', '1 filtered'));
    }
    const rowsBox = $('#ragRows');
    // the prompt keeps one slot per retrieved chunk (K of them), so nothing in the lane moves while a run fills it
    function slotRows(k) {
      rowsBox.replaceChildren(...Array.from({ length: k }, (_, n) => {
        const li = el('li', 'rag-pr-row is-wait');
        li.dataset.slot = `chunk ${n + 1} of ${k}`;
        li.append(el('span', 'rag-pr-ref', '\u00a0'), el('span', 'rag-pr-txt', '\u00a0'), el('span', 'rag-pr-w'));
        return li;
      }));
    }
    function buildRows(Q, k, wait) {
      rowsBox.replaceChildren(...Q.hits.slice(0, k).map(([i, s], n) => {
        const c = CH[i], weak = s < WEAK;
        const li = el('li', `rag-pr-row rag-h${c.h}` + (weak ? ' is-weak' : '') + (wait ? ' is-wait' : ''));
        li.dataset.c = i;
        li.dataset.slot = `chunk ${n + 1} of ${k}`;
        li.append(el('span', 'rag-pr-ref', `${c.sec} · ${s.toFixed(2)}`), el('span', 'rag-pr-txt'), el('span', 'rag-pr-w', weak ? 'weak' : ''));
        return li;
      }));
      $$('.rag-pr-row', rowsBox).forEach(li => fit($('.rag-pr-txt', li), CH[+li.dataset.c].text(days)));
      $('#ragPrQ').textContent = Q.q;
    }
    const ansEl = $('#ragAns'), caret = $('#ragCaret'), srcBox = $('#ragSrcs');
    async function typeAns(me, Q, animate) {
      const [text, src] = Q.ans(K, days);
      srcBox.replaceChildren();
      const done = () => {
        caret.hidden = true;
        ansEl.textContent = text;
        srcBox.replaceChildren(src.length ? el('span', 'src', `HR Policy 2026 ${src.map(i => CH[i].sec).join(', ')}`) : el('span', 'rag-nosrc', 'No permitted source'));
        ansEl.parentElement.classList.remove('is-empty');
      };
      if (!animate || reduced) { done(); return true; }
      ansEl.textContent = '';
      ansEl.parentElement.classList.remove('is-empty');
      caret.hidden = false;
      for (const t of tokenize(text)) {
        if (me !== run) return false;
        ansEl.textContent += t;
        await sleep(24);
      }
      if (me !== run) return false;
      done();
      return true;
    }
    function clearAnswer() { ansEl.textContent = ''; caret.hidden = true; srcBox.replaceChildren(); ansEl.parentElement.classList.add('is-empty'); }
    function clearAsk() {
      gLinks.replaceChildren();
      starG.style.display = 'none';
      starFor = -1;
      clearPts();
      fit($('#ragQText'), '');
      paintCells(cellsQ, null);
      $('#ragFind').replaceChildren();
      slotRows(K);
      $('#ragPrQ').textContent = '';
      clearAnswer();
      resetRail(askRail);
    }
    function renderAsk() {
      const Q = QS[qi];
      fit($('#ragQText'), Q.q, true);
      paintCells(cellsQ, qVec(qi));
      placeStar(Q, false);
      drawLinks(Q, K, false);
      markPts(Q, K);
      setFind(Q, K);
      buildRows(Q, K, false);
      typeAns(run, Q, false);
      stage(askRail, null);
      askRail.classList.remove('is-run');
    }
    function renderFinal() { cancelFly(); renderIngest(); renderAsk(); }

    async function playAsk(me, fromSearch) {
      const Q = QS[qi];
      if (!fromSearch) {
        clearAsk();
        stage(askRail, 'q');
        const qt = $('#ragQText');
        fit(qt, Q.q, true);
        qt.animate([{ opacity: 0, transform: 'translateX(-8px)' }, { opacity: 1, transform: 'none' }], { duration: 360, easing: 'ease-out' });
        if (!(await step(me, 520))) return false;
        stage(askRail, 'emb');
        const v = qVec(qi);
        for (let k = 1; k <= 6; k++) { paintCells(cellsQ, v, k * 4); if (!(await step(me, 55))) return false; }
        if (!(await step(me, 220))) return false;
        stage(askRail, 'search');
        const ok = await flyTo(el('i', 'rag-pkt is-q'), endOf($('#ragCellsQ'), false), mapXY(...Q.star), 640, 36);
        if (!ok || !(await step(me, 0))) return false;
        placeStar(Q, true);
        if (!(await step(me, 320))) return false;
      } else {
        gLinks.replaceChildren();
        clearPts();
        slotRows(K);
        clearAnswer();
        stage(askRail, 'search');
        ripple(starG, 0, 0);
        if (!(await step(me, 200))) return false;
      }
      drawLinks(Q, K, true);
      markPts(Q, K);
      setFind(Q, K);
      if (!(await step(me, 460 + (K + (Q.blocked ? 1 : 0)) * 130))) return false;
      // the chosen chunks lift out of the map into the prompt
      stage(askRail, 'prompt');
      buildRows(Q, K, true);
      const rows = [...rowsBox.children];
      rows.forEach((row, n) => {
        const c = CH[+row.dataset.c], a = mapXY(c.x, c.y), rb = relBox(row);
        flyTo(el('span', `rag-lift rag-h${c.h}`, c.sec), { x: a.x - 22, y: a.y - 12 }, { x: rb.x + 8, y: rb.y + 2 }, 620, 50, n * 120)
          .then(landedOk => { if (landedOk && me === run) { row.classList.remove('is-wait'); row.classList.add('is-fill'); } });
      });
      if (!(await step(me, 640 + rows.length * 120 + 160))) return false;
      rows.forEach(r => { if (r.classList.contains('is-wait')) r.classList.replace('is-wait', 'is-fill'); });
      stage(askRail, 'model');
      if (!(await step(me, 800))) return false;
      stage(askRail, 'ans');
      if (!(await typeAns(me, Q, true))) return false;
      stage(askRail, null);
      askRail.classList.remove('is-run');
      return true;
    }

    /* ---------- controls */
    const chips = $$('.rag-chip'), kBtns = $$('.rag-k .seg-btn');
    const syncChips = () => chips.forEach(b => { const on = +b.dataset.q === qi; b.classList.toggle('is-on', on); b.setAttribute('aria-pressed', String(on)); });
    const syncK = () => kBtns.forEach(b => { const on = +b.dataset.k === K; b.classList.toggle('is-on', on); b.setAttribute('aria-pressed', String(on)); });
    function begin() { const me = ++run; cancelFly(); if (!ingested) renderIngest(); return me; }
    chips.forEach(b => b.addEventListener('click', () => {
      qi = +b.dataset.q;
      syncChips();
      const me = begin();
      if (reduced) { renderAsk(); return; }
      playAsk(me, false);
    }));
    kBtns.forEach(b => b.addEventListener('click', () => {
      K = +b.dataset.k;
      syncK();
      const me = begin();
      if (reduced) { renderAsk(); return; }
      playAsk(me, starFor === qi);
    }));
    $('#ragUpd').addEventListener('click', async () => {
      days = days === 24 ? 25 : 24;
      qi = 0;
      syncChips();
      const me = begin();
      syncDoc();
      $('#ragUpdNote').textContent = days === 25 ? 'Upserted by id: §4.1 replaced in place. No retraining.' : 'Upserted again: back to 24 days. No retraining.';
      if (reduced) { renderIngest(); renderAsk(); return; }
      clearAsk();
      // only the changed chunk goes through the lane again, and lands on the same id
      stage(inRail, 'doc');
      nudge($('#ragDoc'));
      if (!(await step(me, 450))) return;
      stage(inRail, 'chunk');
      if (!(await step(me, 350))) return;
      stage(inRail, 'embed');
      showEmbed(0, true);
      if (!(await step(me, 1000))) return;
      stage(inRail, 'store');
      const ok = await flyTo(el('i', 'rag-pkt rag-h1'), endOf($('#ragCellsIn'), true), mapXY(CH[0].x, CH[0].y), 600, 36);
      if (!ok || !(await step(me, 0))) return;
      ripple(pts[0].g, 0, 0);
      pop(pts[0].pop);
      if (!(await step(me, 450))) return;
      tiles[0].classList.remove('is-cur');
      stage(inRail, null);
      inRail.classList.remove('is-run');
      await playAsk(me, false);
    });

    hooks.rag = () => {
      sizeRails();
      if (reduced) { run++; renderFinal(); return; }
      const me = ++run;
      cancelFly();
      clearAsk();
      (async () => { if (await playIngest(me)) await playAsk(me, false); })();
    };
    let ragResize = 0;
    addEventListener('resize', () => { clearTimeout(ragResize); ragResize = setTimeout(() => { sizeRails(); refit(); }, 160); });
    sizeRails();
    renderFinal();
    document.fonts?.ready.then(() => { sizeRails(); refit(); });
  }
  /* BUILD:rag END */

  /* BUILD:agents START */
  /* ------------------------------------------------------------------ 15: multi-agent orchestration */
  // A LangGraph supervisor classifies each request by type, sends it down the matching branch, takes the agent's
  // result back and decides again: another agent, or finish. Four example requests, the same ones slides 16 and 17
  // reuse; the two-hop "staging feels slow" run (the one slide 17 traces) plays on arrival.
  const AG_NAME = { rag: 'RAG agent', infra: 'Infra agent', gen: 'General agent' };
  const AG_NODE = { rag: 'rag_agent', infra: 'infra_agent', gen: 'general_agent' };
  const AG_TYPE = { rag: 'Knowledge question', infra: 'Infrastructure task', gen: 'Anything else' };
  const AG_RUNS = [
    { key: 'vpn', label: 'VPN policy', req: 'What is our VPN policy for contractors?',
      answer: 'Managed laptops only, and access ends on the contract end date.', cite: 'Security §3.2, §3.4',
      hops: [{ a: 'rag', tool: 'chroma', did: '<b>RAG agent</b> · ChromaDB: Security §3.2, §3.4', sources: ['Security §3.2', 'Security §3.4'] }] },
    { key: 'restart', label: 'Restart billing', req: 'Restart the billing service on staging',
      answer: 'Done: an admin approved, and billing on staging restarted with 2 of 2 replicas ready.',
      hops: [{ a: 'infra', tool: 'restart', did: '<b>Infra agent</b> · restart_service: 2 of 2 ready', calls: ['restart_service(staging, billing)'],
        gate: { ask: 'An admin must approve this restart on staging.', wait: '<b>Infra agent</b> · restart waits for an admin', ok: '<b>Admin</b> · approved · the run resumes' } }] },
    { key: 'farewell', label: 'Farewell note', req: 'Write a farewell note for Ravi from the team', why: ' (drafting)',
      answer: 'Ravi, thank you for four brilliant years. We will miss your calm in every release week.',
      hops: [{ a: 'gen', tool: 'draft', did: '<b>General agent</b> · drafts the note, no tools' }] },
    { key: 'slow', label: 'Staging slow', req: 'Staging feels slow: check it and tell me what the runbook says',
      answer: 'CPU-bound: p95 2.4 s, CPU 92%. Runbook: scale out above 85% CPU for 10 min.', cite: 'Scaling §2',
      hops: [
        { a: 'infra', tool: 'metrics', did: '<b>Infra agent</b> · get_metrics: p95 2.4 s, CPU 92%', calls: ['get_metrics(staging, checkout)'] },
        { a: 'rag', pre: 'Still needs the runbook: ', log: 'needs the runbook', tool: 'chroma', did: '<b>RAG agent</b> · ChromaDB: Scaling §2, Staging §1', sources: ['Scaling §2', 'Staging §1'] },
      ] },
  ];
  const AG_STOP = Symbol('stop'), AG_AWAY = Symbol('away');
  const agDia = $('#agDia'), agSvg = $('#agEdges'), agSup = $('#agSup'), agRead = $('#agRead'), agLog = $('#agLog');
  const agReqBox = $('#agReqBox'), agAnsBox = $('#agAnsBox'), agAns = $('#agAns'), agAnsTag = $('.ag-tag', agAnsBox), agRulesBox = $('#agents .ag-rules');
  const agNodes = Object.fromEntries($$('.ag-node', agDia).map(n => [n.dataset.a, n]));
  const agRules = Object.fromEntries($$('.ag-rule', agDia).map(n => [n.dataset.a, n]));
  const agState = Object.fromEntries($$('#agState dd').map(d => [d.dataset.k, d]));
  let agKey = 'slow', agRun = 0;

  // request chips, each with a dot for every agent the request will use
  const agChips = AG_RUNS.map(r => {
    const b = el('button', 'ag-chip');
    b.type = 'button';
    const dots = el('span', 'ag-chip-dots');
    [...new Set(r.hops.map(h => h.a))].forEach(a => dots.append(el('i', 'ag-c-' + a)));
    b.append(dots, r.label);
    b.setAttribute('aria-label', `${r.req} (uses ${r.hops.map(h => AG_NAME[h.a]).join(', then ')})`);
    b.addEventListener('click', () => agPlay(r.key));
    return b;
  });
  $('#agChips').append(...agChips);
  $('#agReplay').addEventListener('click', () => agPlay(agKey));

  // edges: one arrow down to each agent and one back up, drawn from the layout (canvas px)
  const AG_SVGNS = 'http://www.w3.org/2000/svg';
  const agSvgEl = (tag, cls) => { const n = document.createElementNS(AG_SVGNS, tag); n.setAttribute('class', cls); return n; };
  const agEdges = {};
  ['in', 'out', 'rag-down', 'rag-up', 'infra-down', 'infra-up', 'gen-down', 'gen-up'].forEach(k => {
    const g = agSvgEl('g', 'ag-e ag-c-' + (k.includes('-') ? k.split('-')[0] : 'io'));
    const line = agSvgEl('path', 'ag-edge'), head = agSvgEl('path', 'ag-head');
    g.append(line, head);
    agSvg.append(g);
    agEdges[k] = { g, line, head, x: 0, y1: 0, y2: 0 };
  });
  const agPkt = agSvgEl('circle', 'ag-pkt');
  agPkt.setAttribute('r', 7);
  agSvg.append(agPkt);
  const agBox = n => {
    let x = 0, y = 0;
    for (let e = n; e && e !== agDia; e = e.offsetParent) { x += e.offsetLeft; y += e.offsetTop; }
    return { x, y, w: n.offsetWidth, h: n.offsetHeight };
  };
  function agDraw() {
    if (!agDia.offsetWidth || getComputedStyle(agSvg).display === 'none') return;
    agSvg.setAttribute('viewBox', `0 0 ${agDia.offsetWidth} ${agDia.offsetHeight}`);
    const sup = agBox(agSup), req = agBox(agReqBox), ans = agBox(agAnsBox);
    const top = sup.y, bot = sup.y + sup.h;
    const set = (k, x, y1, y2) => {   // a vertical arrow from y1 to y2
      const d = Math.sign(y2 - y1), e = agEdges[k];
      Object.assign(e, { x, y1, y2 });
      e.line.setAttribute('d', `M${x} ${y1} L${x} ${y2 - d * 8}`);
      e.head.setAttribute('d', `M${x - 6} ${y2 - d * 10} L${x + 6} ${y2 - d * 10} L${x} ${y2} Z`);
    };
    set('in', req.x + 44, req.y + req.h + 3, top - 3);
    set('out', ans.x + ans.w - 44, top - 3, ans.y + ans.h + 3);
    for (const a of ['rag', 'infra', 'gen']) {
      const n = agBox(agNodes[a]);
      set(a + '-down', n.x + 30, bot + 3, n.y - 3);
      set(a + '-up', n.x + n.w - 30, n.y - 3, bot + 3);
    }
  }
  addEventListener('resize', () => setTimeout(agDraw, 200));
  document.fonts?.ready.then(agDraw);
  agDraw();

  // the packet: a glowing dot that rides one edge
  function agTravel(k, ms, me) {
    const e = agEdges[k];
    e.g.classList.add('is-run');
    agPkt.setAttribute('class', 'ag-pkt on ' + [...e.g.classList].find(c => c.startsWith('ag-c-')));
    return new Promise((res, rej) => {
      const t0 = performance.now();
      const tick = now => {
        if (me !== agRun) { rej(AG_STOP); return; }
        const t = clamp((now - t0) / ms), ease = t < 0.5 ? 2 * t * t : 1 - (2 - 2 * t) ** 2 / 2;
        agPkt.setAttribute('cx', e.x);
        agPkt.setAttribute('cy', e.y1 + (e.y2 - e.y1) * ease);
        if (t < 1) { requestAnimationFrame(tick); return; }
        agEdgeDone(k);
        agPkt.classList.remove('on');
        res();
      };
      requestAnimationFrame(tick);
    });
  }
  const agEdgeDone = k => { agEdges[k].g.classList.remove('is-run'); agEdges[k].g.classList.add('is-done'); };
  async function agWait(ms, me) {
    await sleep(ms);
    if (me !== agRun) throw AG_STOP;
    if (!isActive('agents')) throw AG_AWAY;
  }

  // shared state, readout, log
  const agStr = s => `<span class="ag-q">"${s}"</span>`;
  const agList = (xs, quote = true) => '[' + xs.map(x => `<span class="ag-q">${quote ? `"${x}"` : x}</span>`).join(', ') + ']';
  function agSet(k, v, flash) {
    const dd = agState[k];
    if (dd.innerHTML === v) return;
    dd.innerHTML = v;
    if (flash && !reduced) { dd.classList.remove('flash'); void dd.offsetWidth; dd.classList.add('flash'); }
  }
  function agSay(htmlText, flash, thinking) {
    agRead.innerHTML = htmlText;
    agRead.classList.toggle('ag-thinking', !!thinking);
    if (flash && !reduced) { agRead.classList.remove('pop'); void agRead.offsetWidth; agRead.classList.add('pop'); }
  }
  function agLogLine(who, text, cls) {
    const li = el('li', 'ag-c-' + who + (cls ? ' ' + cls : ''));
    li.innerHTML = `<span class="ag-n">${cls ? 'i' : agLog.querySelectorAll('li:not(.ag-note)').length + 1}</span><span>${text}</span>`;
    agLog.append(li);
  }
  const agLight = a => {
    Object.entries(agRules).forEach(([k, r]) => r.classList.toggle('is-lit', k === a));
    agRulesBox.classList.toggle('has-lit', !!a);
  };
  // a change waits at a checkpoint until an admin approves it: the presenter clicks, as the admin would
  function agApproval(ask, me) {
    agAnsTag.textContent = 'Approval';
    agAnsBox.classList.add('is-wait');
    const btn = el('button', 'btn btn-small ag-approve', 'Approve as admin');
    btn.type = 'button';
    agAns.replaceChildren(el('span', null, ask), btn);
    return new Promise((res, rej) => {
      const end = () => { clearInterval(poll); agAnsTag.textContent = 'Answer'; agAnsBox.classList.remove('is-wait'); };
      const poll = setInterval(() => {
        if (me !== agRun) { end(); rej(AG_STOP); } else if (!isActive('agents')) { end(); rej(AG_AWAY); }
      }, 150);
      btn.addEventListener('click', () => {
        end();
        agAns.innerHTML = '<span class="ag-wait">comes back through the supervisor</span>';
        res();
      }, { once: true });
    });
  }
  const agHot = (a, tool, on) => { const t = tool && $(`[data-tool="${tool}"]`, agNodes[a]); if (t) t.classList.toggle('is-hot', on); };

  // a run is a list of beats; played live they wait and animate, played instantly they give the final state
  function agBeats(run) {
    const st = { steps: [], calls: [], sources: [] };
    const used = [];
    const B = [];
    B.push({ ms: 340, go: live => {
      agLog.replaceChildren();
      Object.values(agEdges).forEach(e => e.g.classList.remove('is-run', 'is-done'));
      Object.values(agNodes).forEach(n => n.classList.remove('is-on', 'is-wait'));
      $$('.ag-tools span', agDia).forEach(s => s.classList.remove('is-hot'));
      agLight(null);
      agSup.classList.remove('is-on');
      $('#agReq').textContent = run.req;
      agAns.innerHTML = '<span class="ag-wait">comes back through the supervisor</span>';
      agAnsTag.textContent = 'Answer';
      agAnsBox.classList.remove('is-done', 'is-wait');
      agSay('Reads each request and picks the agent for it', false);
      agSet('request', `"${run.req}"`, live);
      agSet('next', '<span class="ag-q">null</span>', false);
      agSet('steps', '[]', false);
      agSet('tool_calls', '[]', false);
      agSet('sources', '[]', false);
      agSet('status', agStr('received'), live);
    } });
    const classify = () => ({ ms: 540, go: () => { agSup.classList.add('is-on'); agSay('Classifying the request', true, true); agSet('status', agStr('routing'), true); } });
    B.push({ edge: 'in', ms: 0, go: () => {} });
    run.hops.forEach((h, i) => {
      B.push(classify());
      B.push({ ms: 1000, go: live => {
        agSay(`${h.pre || 'Type: '}<span class="ag-type ag-c-${h.a}">${AG_TYPE[h.a]}</span>${run.why || ''} → ${AG_NAME[h.a]}`, live);
        agLight(h.a);
        used.push(h.a);
        agSet('next', `<span class="ag-q ag-k ag-c-${h.a}">"${AG_NODE[h.a]}"</span>`, live);
        agLogLine('sup', `<b class="ag-c-sup">Supervisor</b> → ${AG_NAME[h.a]} · ${h.log || AG_TYPE[h.a].toLowerCase()}`);
      } });
      B.push({ edge: h.a + '-down', ms: 0, pre: () => { agSup.classList.remove('is-on'); agSet('status', agStr('running ' + AG_NODE[h.a]), true); },
        go: () => { agNodes[h.a].classList.add('is-on'); } });
      B.push({ ms: 300, go: () => { agHot(h.a, h.tool, true); } });
      if (h.gate) B.push({ ms: 400, go: async (live, me) => {
        agLogLine(h.a, h.gate.wait);
        agSet('status', agStr('waiting for admin'), live);
        agNodes[h.a].classList.add('is-wait');
        if (live) await agApproval(h.gate.ask, me);
        agNodes[h.a].classList.remove('is-wait');
        agLogLine('adm', h.gate.ok);
        agSet('status', agStr('running ' + AG_NODE[h.a]), live);
      } });
      B.push({ ms: 950, go: live => {
        if (h.calls) { st.calls.push(...h.calls); agSet('tool_calls', agList(st.calls, false), live); }
        if (h.sources) { st.sources.push(...h.sources); agSet('sources', agList(st.sources), live); }
        agLogLine(h.a, h.did);
      } });
      B.push({ edge: h.a + '-up', ms: 0, pre: () => { agHot(h.a, h.tool, false); agNodes[h.a].classList.remove('is-on'); },
        go: live => { st.steps.push(AG_NODE[h.a]); agSet('steps', '[' + st.steps.map(s => `<span class="ag-q ag-k ag-c-${s.split('_')[0].replace('general', 'gen')}">"${s}"</span>`).join(', ') + ']', live); } });
    });
    B.push(classify());
    B.push({ ms: 650, go: live => {
      agSay('Done → answer', live);
      agLight(null);
      used.forEach(a => agRules[a].classList.add('is-lit'));
      agSet('next', '<span class="ag-q ag-k">"FINISH"</span>', live);
      agLogLine('sup', '<b class="ag-c-sup">Supervisor</b> → finish · answer sent');
    } });
    B.push({ edge: 'out', ms: 0, pre: () => {}, go: () => { agSup.classList.remove('is-on'); } });
    B.push({ ms: 0, go: async (live, me) => {
      agAnsBox.classList.add('is-done');
      agAns.replaceChildren();
      if (live) {
        for (const t of tokenize(run.answer)) {
          if (me !== agRun) throw AG_STOP;
          agAns.append(el('span', 'new', t));
          await sleep(24);
        }
      } else agAns.textContent = run.answer;
      if (run.cite) agAns.append(el('span', 'ag-cite', run.cite));
      agSet('status', agStr('done'), live);
      if (run.note) agLogLine('io', run.note, 'ag-note');
    } });
    return B;
  }
  function agMark() {
    agChips.forEach((c, i) => { const on = AG_RUNS[i].key === agKey; c.classList.toggle('is-on', on); c.setAttribute('aria-pressed', String(on)); });
  }
  function agFinal(run) {
    const me = ++agRun;
    agPkt.classList.remove('on');
    for (const b of agBeats(run)) { b.pre?.(); if (b.edge) agEdgeDone(b.edge); b.go(false, me); }
  }
  async function agPlay(key) {
    agKey = key;
    agMark();
    agDraw();
    const run = AG_RUNS.find(r => r.key === key);
    if (reduced || !isActive('agents')) { agFinal(run); return; }
    const me = ++agRun;
    try {
      for (const b of agBeats(run)) {
        b.pre?.();
        if (b.edge) await agTravel(b.edge, b.edge.includes('-') ? 520 : 420, me);
        await b.go(true, me);
        if (b.ms) await agWait(b.ms, me);
        if (!isActive('agents')) throw AG_AWAY;
      }
    } catch (err) {
      if (err === AG_AWAY && me === agRun) agFinal(run);   // left the slide mid-run: show how it ends
      else if (err !== AG_STOP && err !== AG_AWAY) throw err;
    }
  }
  agFinal(AG_RUNS[3]);
  agMark();
  // On a phone this slide is taller than the deck's 35% visibility threshold can ever reach, so it also enters
  // (headline and run) once a sixth of it is on screen.
  if ('IntersectionObserver' in window) {
    const agSlide = $('#agents');
    new IntersectionObserver(([e]) => {
      if (isDeck()) return;
      if (e.intersectionRatio >= 0.16 && !visible.has('agents')) { visible.add('agents'); cur = slides.indexOf(agSlide); enter(cur); }
      else if (!e.isIntersecting) visible.delete('agents');
    }, { threshold: [0, 0.16] }).observe(agSlide);
  }
  hooks.agents = () => {
    agDraw();
    if (reduced) { agFinal(AG_RUNS.find(r => r.key === agKey)); return; }
    const me = ++agRun;   // a fresh run once the slide has settled
    setTimeout(() => { if (me === agRun && isActive('agents')) agPlay(agKey); }, 900);
  };
  /* BUILD:agents END */

  /* BUILD:hitl START */
  /* ------------------------------------------------------------------ 16: human in the loop */
  // The run plays up to the checkpoint and stops there. Nothing happens until the presenter approves, edits or
  // rejects; each choice resumes the same run from checkpoint 7. "Restart the server" wipes the screen and
  // restores the run from the saved checkpoint, still waiting.
  const HITL_AFTER = { 3: ['460 ms', '66%'], 4: ['380 ms', '54%'], 5: ['340 ms', '45%'], 6: ['320 ms', '38%'] };
  const hitlSteps = $$('#hitlSteps .hitl-step');
  const hitlCard = $('#hitlCard'), hitlLog = $('#hitlLog'), hitlTrack = $('#hitlFill').parentElement, hitlOverlay = $('#hitlOverlay');
  const hitlBtn = { approve: $('#hitlApprove'), edit: $('#hitlEdit'), reject: $('#hitlReject'), restart: $('#hitlRestart') };
  let hitlRun = 0, hitlState = 'idle', hitlEditing = false, hitlReplicas = 4;
  const hitlPause = ms => sleep(reduced ? 0 : ms);

  function hitlMark(i, cls) {
    hitlSteps[i].classList.remove('done', 'wait', 'stop', 'skip');
    if (cls) hitlSteps[i].classList.add(cls);
  }
  // --p on the track drives the emerald fill; --k (set on a rejection) turns the rest of the track into a skipped stretch
  const hitlReach = i => hitlTrack.style.setProperty('--p', i / (hitlSteps.length - 1));
  function hitlLine(time, text, kind) {
    const li = el('li', kind || null);
    li.append(el('time', null, time), el('span', null, text));
    hitlLog.append(li);
    return li;
  }
  function hitlEnable(on) {
    for (const b of Object.values(hitlBtn)) b.disabled = !on;
  }
  function hitlSetEditing(on) {
    hitlEditing = on;
    $('#hitlNew').hidden = on;
    $('#hitlStepper').hidden = !on;
    $('#hitlEditVal').textContent = hitlReplicas;
    $('#hitlLess').disabled = hitlReplicas <= 3;
    $('#hitlMore').disabled = hitlReplicas >= 6;
    hitlBtn.approve.textContent = on && hitlReplicas !== 4 ? `Approve ${hitlReplicas} replicas` : 'Approve';
    hitlBtn.edit.textContent = on ? 'Cancel edit' : 'Edit';
    hitlBtn.reject.hidden = on;
  }
  function hitlReset() {
    hitlRun++;
    hitlState = 'idle';
    hitlReplicas = 4;
    hitlSteps.forEach((_, i) => hitlMark(i));
    hitlReach(0);   // a stale --k is harmless: --p stays at or below 3/7 until the next decision sets or clears it
    $('#hitlDecideLabel').textContent = 'A person decides';
    $('#hitlDecideNote').textContent = 'approve, edit, reject';
    $('#hitlExecNote').textContent = 'apply the change';
    $('#hitlNew').textContent = '4';
    hitlLog.replaceChildren();
    hitlCard.classList.remove('waiting', 'decided');
    hitlOverlay.hidden = true;
    $('#hitlKicker').textContent = 'Approval needed · Infra agent';
    $('#hitlActions').hidden = false;
    hitlBtn.restart.style.visibility = '';
    $('#hitlDecision').hidden = true;
    hitlSetEditing(false);
    hitlEnable(false);
  }
  function hitlWait(restored) {
    for (let i = 0; i < 3; i++) hitlMark(i, 'done');
    hitlMark(3, 'wait');
    hitlReach(3);
    hitlCard.classList.add('waiting');
    hitlState = 'waiting';
    const text = restored ? 'Restored after the restart: still paused at checkpoint 7' : 'Paused at checkpoint 7: nothing has changed in production';
    const status = $('li.status', hitlLog);
    if (status) status.lastChild.textContent = text; else hitlLine('10:41', text, 'status');
    hitlEnable(true);
  }
  async function hitlToCheckpoint() {
    const me = ++hitlRun;
    for (let i = 0; i < 3; i++) {
      await hitlPause(i ? 520 : 700);
      if (me !== hitlRun || !isActive('hitl')) return;
      hitlMark(i, 'done');
      hitlReach(i);
    }
    await hitlPause(520);
    if (me !== hitlRun || !isActive('hitl')) return;
    hitlWait(false);
  }
  async function hitlDecide(kind) {
    if (hitlState !== 'waiting') return;
    const me = ++hitlRun;
    const n = kind === 'edit' ? hitlReplicas : 4;
    hitlState = 'resuming';
    hitlEnable(false);
    hitlSetEditing(false);
    $('#hitlNew').textContent = String(n);
    hitlCard.classList.remove('waiting');
    const status = $('li.status', hitlLog);
    if (status) { status.className = ''; status.lastChild.textContent = 'Paused at checkpoint 7 and waited for a person'; }
    const reject = kind === 'reject';
    const [p95, cpu] = HITL_AFTER[n] || HITL_AFTER[4];
    const lines = reject ? [
      ['10:42', 'Rejected by the on-call engineer', 'no'],
      ['10:42', 'Resumed at checkpoint 7: no change made', ''],
      ['10:42', 'The agent will alert again if p95 stays above 2 s', ''],
      ['10:42', 'Reported back; the decision is in the trace', 'ok'],
    ] : [
      ['10:42', kind === 'edit' ? `Edited by the on-call engineer: ${n} replicas` : 'Approved by the on-call engineer', 'ok'],
      ['10:42', `Resumed at checkpoint 7: scaled 2 → ${n} replicas`, 'ok'],
      ['10:47', `Verified: p95 ${p95}, CPU ${cpu}`, 'ok'],
      ['10:47', kind === 'edit' ? 'Reported back; the edit is in the trace' : 'Reported back; the decision is in the trace', 'ok'],
    ];
    $('#hitlDecideLabel').textContent = reject ? 'Rejected' : kind === 'edit' ? 'Edited' : 'Approved';
    $('#hitlDecideNote').textContent = kind === 'edit' ? `${n} replicas, not 4` : 'on-call engineer';
    const stamp = $('#hitlDecision');
    stamp.textContent = reject ? 'Rejected by the on-call engineer · no change · 10:42 · logged' : kind === 'edit' ? 'Approved with an edit by the on-call engineer · 10:42 · logged' : 'Approved by the on-call engineer · 10:42 · logged';
    stamp.className = 'hitl-decision' + (reject ? ' no' : '');
    $('#hitlActions').hidden = true;
    stamp.hidden = false;
    $('#hitlKicker').textContent = 'Decision recorded · Infra agent';
    hitlBtn.restart.style.visibility = 'hidden';   // keeps its space, so the card does not shrink
    hitlCard.classList.add('decided');
    $('#hitlExecNote').textContent = reject ? 'skipped' : `scaled 2 → ${n}`;
    hitlMark(3, 'done');
    if (reject) hitlTrack.style.setProperty('--k', 4 / (hitlSteps.length - 1)); else hitlTrack.style.removeProperty('--k');
    for (let k = 0; k < 4; k++) {
      await hitlPause(k ? 650 : 250);
      if (me !== hitlRun) return;
      const i = 4 + k;
      hitlMark(i, reject && (i === 5 || i === 6) ? 'skip' : reject && i === 4 ? 'stop' : 'done');
      hitlReach(i);
      hitlLine(...lines[k]);
    }
    hitlState = 'done';
  }
  async function hitlRestart() {
    if (hitlState !== 'waiting') return;
    const me = ++hitlRun;
    hitlState = 'restarting';
    hitlSetEditing(false);
    hitlEnable(false);
    hitlCard.classList.remove('waiting');
    hitlOverlay.classList.remove('restored');
    $('#hitlOverlayText').textContent = 'Server restarting…';
    hitlOverlay.hidden = false;
    hitlSteps.forEach((_, i) => hitlMark(i));
    hitlReach(0);
    await hitlPause(1300);
    if (me !== hitlRun) return;
    hitlOverlay.classList.add('restored');
    $('#hitlOverlayText').textContent = 'Loaded checkpoint 7 from the database';
    for (let i = 0; i < 3; i++) { hitlMark(i, 'done'); hitlReach(i); await hitlPause(150); }
    await hitlPause(800);
    if (me !== hitlRun) return;
    hitlOverlay.hidden = true;
    hitlWait(true);
  }
  // an edit that ends where the proposal started (4) is a plain approval
  hitlBtn.approve.addEventListener('click', () => hitlDecide(hitlEditing && hitlReplicas !== 4 ? 'edit' : 'approve'));
  hitlBtn.edit.addEventListener('click', () => {
    if (hitlState !== 'waiting') return;
    if (!hitlEditing) hitlReplicas = 4;   // the stepper starts at the proposal
    hitlSetEditing(!hitlEditing);
  });
  hitlBtn.reject.addEventListener('click', () => hitlDecide('reject'));
  hitlBtn.restart.addEventListener('click', hitlRestart);
  $('#hitlLess').addEventListener('click', () => { hitlReplicas = Math.max(3, hitlReplicas - 1); hitlSetEditing(true); });
  $('#hitlMore').addEventListener('click', () => { hitlReplicas = Math.min(6, hitlReplicas + 1); hitlSetEditing(true); });
  $('#hitlReplay').addEventListener('click', () => { hitlReset(); hitlToCheckpoint(); });
  hitlReset();
  // coming back to a run that is waiting for a decision keeps it waiting; anything else replays from the start
  hooks.hitl = () => { if (hitlState !== 'waiting') { hitlReset(); hitlToCheckpoint(); } };
  /* BUILD:hitl END */

  /* BUILD:observe START */
  /* ------------------------------------------------------------------ 17: LangSmith observability */
  // Five recent runs, one for each kind of task on slide 15, traced the way LangSmith shows them (illustrative).
  // The run picker says which agents each task used, the route line spells out the hops, every step carries its
  // agent's colour, and supervisor steps say where they routed. The two-hop "staging feels slow" run streams in on
  // arrival; its failed twin shows the metrics tool timing out after 10 s and a retry succeeding.
  const OBS_TYPE = { chain: 'Chain', llm: 'LLM', tool: 'Tool', ret: 'Retriever', human: 'Approval' };
  const OBS_AGENT = { sup: 'Supervisor', infra: 'Infra agent', rag: 'RAG agent', gen: 'General agent' };
  const OBS_SHORT = { sup: 'Supervisor', infra: 'Infra', rag: 'RAG', gen: 'General' };
  // the model the harness chose for an LLM step: a small fast one for routing and short replies, a larger one for
  // tool use and grounded answers, Gemma for drafting (illustrative choices of open-weight models we can host)
  const obsLlm = (model, temp, inTok, outTok) => ({ model, tok: inTok + outTok,
    meta: `temperature <b>${temp}</b> · <b>${fmt(inTok)}</b> tokens in, <b>${fmt(outTok)}</b> out` });
  const OBS_EMBED = 'MiniLM-L6-v2';
  const OBS_PICK = 'messages: the request\nchoose one of: rag_agent · infra_agent · general_agent · FINISH';
  const OBS_SLOW_REQ = 'Staging feels slow: check it and tell me what the runbook says';
  const OBS_SLOW_ANSWER = 'CPU-bound: p95 2.4 s, CPU 92% on 2 replicas. Runbook: scale out above 85% CPU for 10 min; on staging an admin approves the change.';
  const OBS_CHUNKS = {
    slow: [['0.91', 'Scaling §2', 'Scale out above 85% CPU for 10 min'], ['0.84', 'Staging §1', 'An admin approves staging changes'],
      ['0.77', 'Alerts §3', 'p95 over 2 s for 5 min pages on-call'], ['0.62', 'Postmortem', 'Slow checkout: an under-sized cache']],
    vpn: [['0.89', 'Security §3.2', 'Contractor VPN: managed laptops only'], ['0.81', 'Security §3.4', 'Access ends on the contract end date'],
      ['0.66', 'Onboarding §2', 'Managers request contractor access'], ['0.58', 'IT FAQ', 'Setting up the VPN client']],
  };
  const obsRoot = (req, answer, total, meta) => ({ name: 'supervisor graph', type: 'chain', depth: 0, t0: 0, t1: total, input: `"${req}"`, output: answer, meta });
  const obsSup = (to, kind, t0, t1, tok, inTok) => ({ name: `supervisor → ${to.replace('_agent', '')}`, type: 'llm', agent: 'sup', depth: 1, t0, t1,
    ...obsLlm('gpt-oss-20b', 0, inTok, tok - inTok),
    input: to === 'finish' ? 'messages: the request + every agent result' : OBS_PICK,
    output: to === 'finish' ? 'next: "FINISH"\nthe request is answered' : `type: ${kind}\nnext: "${to}"` });
  function obsSlow(fail) {
    const S = fail ? 10.02 : 0, t = x => x + S;     // everything after the metrics call shifts by the timeout
    const spans = [
      obsRoot(OBS_SLOW_REQ, OBS_SLOW_ANSWER, t(4.40), 'user <b>on-call engineer</b> · department <b>engineering</b> · thread <b>infra-0412</b>'),
      obsSup('infra_agent', 'infrastructure task', 0, 0.42, 312, 280),
      { name: 'infra_agent', type: 'chain', agent: 'infra', depth: 1, t0: 0.42, t1: t(2.10),
        input: `"${OBS_SLOW_REQ}"`, output: 'p95 2.4 s, CPU 92%, 2 replicas: looks CPU-bound', meta: '2 model calls · 1 tool call · <b>950</b> tokens' },
      { name: 'choose a tool', type: 'llm', agent: 'infra', depth: 2, t0: 0.44, t1: 0.98, ...obsLlm('gpt-oss-120b', 0, 372, 38),
        input: 'system: You are the Infra agent. Use only the listed tools.\ntools: get_metrics, get_logs, restart_service (staging only, after an admin approves)', output: 'call get_metrics(env="staging", service="checkout")' },
      fail
        ? { name: 'get_metrics', type: 'tool', agent: 'infra', depth: 2, t0: 1.00, t1: 11.00, err: true,
            input: 'get_metrics(env="staging", service="checkout")', error: 'TimeoutError: the metrics API did not answer within 10 s',
            diag: '<b>The slow step was the metrics API, not the model.</b> It timed out after 10 s; the automatic retry answered in 0.35 s. Fix or alert on the API before blaming the model.', meta: 'read-only · runs without approval · attempt <b>1 of 3</b>' }
        : { name: 'get_metrics', type: 'tool', agent: 'infra', depth: 2, t0: 1.00, t1: 1.35,
            input: 'get_metrics(env="staging", service="checkout")', output: '{ "p95_ms": 2400, "cpu": 0.92, "replicas": 2 }', meta: 'read-only · runs without approval (slide 16)' },
      ...(fail ? [{ name: 'get_metrics · retry', type: 'tool', agent: 'infra', depth: 2, t0: 11.02, t1: 11.37,
            input: 'get_metrics(env="staging", service="checkout")', output: '{ "p95_ms": 2400, "cpu": 0.92, "replicas": 2 }', meta: 'read-only · attempt <b>2 of 3</b> · succeeded' }] : []),
      { name: 'read metrics', type: 'llm', agent: 'infra', depth: 2, t0: t(1.37), t1: t(2.08), ...obsLlm('gpt-oss-120b', 0.2, 486, 54),
        input: 'tool result: { "p95_ms": 2400, "cpu": 0.92, "replicas": 2 }\nquestion: is staging healthy?', output: 'Staging checkout: p95 2.4 s, CPU 92% on 2 replicas. Looks CPU-bound.' },
      obsSup('rag_agent', 'knowledge question (the runbook)', t(2.10), t(2.48), 290, 266),
      { name: 'rag_agent', type: 'chain', agent: 'rag', depth: 1, t0: t(2.48), t1: t(4.16),
        input: `"${OBS_SLOW_REQ}"`, output: 'Scale out: CPU is above 85% for over 10 minutes [Scaling §2]; on staging an admin approves it [Staging §1].', meta: '1 search · 1 model call · <b>1,420</b> tokens' },
      { name: 'chromadb.query', type: 'ret', agent: 'rag', depth: 2, t0: t(2.52), t1: t(2.56), chunks: 'slow',
        input: 'query: "staging slow, high CPU: what does the runbook say?"\nwhere: { access: "engineering" } · k: 4', model: OBS_EMBED, meta: 'embedding <b>all-MiniLM-L6-v2</b> · collection <b>runbooks</b> · 4 of 1,812 chunks' },
      { name: 'answer with sources', type: 'llm', agent: 'rag', depth: 2, t0: t(2.60), t1: t(4.12), ...obsLlm('gpt-oss-120b', 0.2, 1236, 184),
        input: 'system: Answer only from the passages and cite them.\n+ 4 passages (980 tokens) + the question', output: 'Scale out: CPU has been above 85% for more than 10 minutes [Scaling §2], and on staging an admin approves the change [Staging §1].' },
      obsSup('finish', '', t(4.16), t(4.37), 180, 150),
    ];
    return { spans, pick: fail ? 4 : spans.findIndex(s => s.chunks) };
  }
  function obsVpn() {
    const req = 'What is our VPN policy for contractors?';
    const answer = 'Managed laptops only; access ends with the contract [Security §3.2, §3.4].';
    const spans = [
      obsRoot(req, answer, 2.31, 'user <b>project manager</b> · department <b>delivery</b> · thread <b>delivery-0233</b>'),
      obsSup('rag_agent', 'knowledge question', 0, 0.36, 296, 268),
      { name: 'rag_agent', type: 'chain', agent: 'rag', depth: 1, t0: 0.36, t1: 2.08, input: `"${req}"`, output: answer, meta: '1 search · 1 model call · <b>1,180</b> tokens' },
      { name: 'chromadb.query', type: 'ret', agent: 'rag', depth: 2, t0: 0.40, t1: 0.44, chunks: 'vpn',
        input: 'query: "VPN policy for contractors"\nwhere: { access: "all staff" } · k: 4', model: OBS_EMBED, meta: 'embedding <b>all-MiniLM-L6-v2</b> · collection <b>policies</b> · 4 of 2,406 chunks' },
      { name: 'answer with sources', type: 'llm', agent: 'rag', depth: 2, t0: 0.48, t1: 2.04, ...obsLlm('gpt-oss-120b', 0.2, 1032, 148),
        input: 'system: Answer only from the passages and cite them.\n+ 4 passages (860 tokens) + the question', output: answer },
      obsSup('finish', '', 2.08, 2.29, 170, 146),
    ];
    return { spans, pick: 3 };
  }
  function obsRestart() {
    const req = 'Restart the billing service on staging';
    const spans = [
      obsRoot(req, 'Done: an admin approved, and billing on staging restarted at 10:12 with 2 of 2 replicas ready.', 3.06, 'user <b>developer</b> · department <b>engineering</b> · thread <b>infra-0409</b>'),
      obsSup('infra_agent', 'infrastructure task', 0, 0.34, 288, 258),
      { name: 'infra_agent', type: 'chain', agent: 'infra', depth: 1, t0: 0.34, t1: 2.84, input: `"${req}"`, output: 'Billing on staging restarted; 2 of 2 replicas ready.', meta: '2 model calls · 1 approval · 1 tool call · <b>638</b> tokens' },
      { name: 'choose a tool', type: 'llm', agent: 'infra', depth: 2, t0: 0.36, t1: 0.86, ...obsLlm('gpt-oss-120b', 0, 364, 38),
        input: 'system: You are the Infra agent. Use only the listed tools.\ntools: get_metrics, get_logs, restart_service (staging only, after an admin approves)', output: 'call restart_service(env="staging", service="billing")' },
      // the run stops at interrupt() and resumes with the admin's decision; the minutes in between are not latency
      { name: 'admin approval', type: 'human', agent: 'infra', depth: 2, t0: 0.88, t1: 0.92,
        input: 'interrupt({ "action": "restart_service", "env": "staging", "service": "billing" })', output: '{ "decision": "approve", "by": "admin" }',
        meta: 'paused at checkpoint <b>3</b> · an <b>admin</b> approved 46 s later · the wait is left out of the timings (slide 16)' },
      { name: 'restart_service', type: 'tool', agent: 'infra', depth: 2, t0: 0.94, t1: 2.38,
        input: 'restart_service(env="staging", service="billing")', output: '{ "status": "restarted", "ready": "2/2" }', meta: 'staging change · ran after an admin approved' },
      { name: 'confirm', type: 'llm', agent: 'infra', depth: 2, t0: 2.40, t1: 2.82, ...obsLlm('gpt-oss-20b', 0.2, 204, 32),
        input: 'tool result: { "status": "restarted", "ready": "2/2" }', output: 'Billing on staging restarted; 2 of 2 replicas ready.' },
      obsSup('finish', '', 2.84, 3.04, 160, 136),
    ];
    return { spans, pick: 4 };
  }
  function obsFarewell() {
    const req = 'Write a farewell note for Ravi from the team';
    const note = 'Ravi, thank you for four brilliant years. We will miss your calm in every release week.';
    const spans = [
      obsRoot(req, note, 3.96, 'user <b>team lead</b> · department <b>engineering</b> · thread <b>team-0187</b>'),
      obsSup('general_agent', 'anything else (drafting)', 0, 0.31, 276, 250),
      { name: 'general_agent', type: 'chain', agent: 'gen', depth: 1, t0: 0.31, t1: 3.71, input: `"${req}"`, output: note, meta: '1 model call · no tools · <b>640</b> tokens' },
      { name: 'draft the note', type: 'llm', agent: 'gen', depth: 2, t0: 0.33, t1: 3.69, ...obsLlm('gemma-3-27b', 0.7, 96, 544),
        input: 'system: You are the General agent.\nuser: Write a farewell note for Ravi from the team', output: note },
      obsSup('finish', '', 3.71, 3.92, 150, 124),
    ];
    return { spans, pick: 3 };
  }
  const OBS_RUNS = [
    { key: 'vpn', label: 'VPN policy', tags: ['rag'], req: 'What is our VPN policy for contractors?', build: obsVpn },
    { key: 'restart', label: 'Restart billing', tags: ['infra'], req: 'Restart the billing service on staging', build: obsRestart },
    { key: 'farewell', label: 'Farewell note', tags: ['gen'], req: 'Write a farewell note for Ravi from the team', build: obsFarewell },
    { key: 'slow', label: 'Staging slow', tags: ['infra', 'rag'], req: OBS_SLOW_REQ, build: () => obsSlow(false) },
    { key: 'fail', label: 'Staging slow', tags: ['infra', 'rag'], fail: true, req: OBS_SLOW_REQ, build: () => obsSlow(true) },
  ];
  const obsApp = $('#observe .obs-app'), obsRows = $('#obsRows'), obsInsp = $('#obsInsp');
  let obsRun = 0, obsKey = 'slow', obsData = null;
  const obsTag = (agent, text) => el('span', `obs-atag a-${agent}`, text);
  const obsSec = (label, ...kids) => { const d = el('div', 'obs-sec'); d.append(el('p', null, label), ...kids); return d; };
  const obsCode = text => el('p', 'obs-code', text);
  function obsSelect(i) {
    $$('.obs-span', obsRows).forEach(b => { const on = +b.dataset.i === i; b.classList.toggle('is-on', on); b.setAttribute('aria-pressed', String(on)); });
    const sp = obsData.spans[i];
    if (!sp) return;
    const head = el('div', 'obs-ih');
    head.append(el('span', `obs-type t-${sp.type}`, OBS_TYPE[sp.type]), el('h3', null, sp.name));
    if (sp.model) head.append(el('span', 'obs-mtag', sp.model));
    const stats = el('p', 'obs-istats');
    if (sp.agent) stats.append(obsTag(sp.agent, OBS_AGENT[sp.agent]));
    stats.append(`${(sp.t1 - sp.t0).toFixed(2)} s · from ${sp.t0.toFixed(2)} s${sp.tok ? ` · ${fmt(sp.tok)} tokens` : ''}`);
    const parts = [head, stats, obsSec('Input', obsCode(sp.input))];
    if (sp.error) {
      parts.push(obsSec('Error', el('p', 'obs-err', sp.error)));
      const d = el('p', 'obs-diag'); d.innerHTML = sp.diag; parts.push(d);
    } else if (sp.chunks) {
      const ol = el('ol', 'obs-chunks');
      for (const [score, src, text] of OBS_CHUNKS[sp.chunks]) { const li = el('li'); li.append(el('b', null, score), el('em', null, src), el('span', null, text)); ol.append(li); }
      parts.push(obsSec('Output · 4 passages', ol));
    } else {
      parts.push(obsSec('Output', obsCode(sp.output)));
    }
    const meta = el('p', 'obs-meta'); meta.innerHTML = sp.meta; parts.push(meta);
    obsInsp.replaceChildren(...parts);
    obsInsp.classList.remove('pop'); void obsInsp.offsetWidth; obsInsp.classList.add('pop');
  }
  function obsRowEl(sp, i, growMs) {
    const li = el('li');
    const b = el('button', `obs-row obs-span t-${sp.type}${sp.err ? ' err' : ''}`);
    b.type = 'button';
    b.dataset.i = i;
    b.setAttribute('aria-pressed', 'false');
    b.setAttribute('aria-label', `${sp.name}${sp.agent ? ', ' + OBS_AGENT[sp.agent] : ''}${sp.model ? ', model ' + sp.model : ''}, ${OBS_TYPE[sp.type]}, ${(sp.t1 - sp.t0).toFixed(2)} seconds${sp.err ? ', failed' : ''}`);
    const name = el('span', 'obs-name');
    name.append(el('b', 'obs-stripe' + (sp.agent ? ' a-' + sp.agent : '')));
    for (let d = 0; d < sp.depth; d++) name.append(el('i'));
    name.append(el('span', 'obs-type', OBS_TYPE[sp.type]), el('span', 'obs-label', sp.name));
    const track = el('span', 'obs-track'), bar = el('span', 'obs-bar');
    bar.style.left = (sp.t0 / obsData.total * 100) + '%';
    bar.style.setProperty('--w', Math.max(0.5, (sp.t1 - sp.t0) / obsData.total * 100) + '%');
    bar.style.setProperty('--grow', Math.max(0.22, growMs / 1000).toFixed(2) + 's');
    track.append(bar);
    b.append(name, el('span', 'obs-model' + (sp.model ? '' : ' none'), sp.model || '–'), track, el('span', 'obs-num', (sp.t1 - sp.t0).toFixed(2) + ' s'), el('span', 'obs-num' + (sp.tok ? '' : ' none'), sp.tok ? fmt(sp.tok) : '–'));
    b.addEventListener('click', () => obsSelect(i));
    li.append(b);
    return li;
  }
  function obsAxis() {
    const T = obsData.total, step = T > 8 ? 5 : 1;
    const ticks = [];
    for (let s = 0; s <= T + 1e-9; s += step) ticks.push(s);
    $('#obsAxis').replaceChildren(...ticks.map(s => {
      const pct = s / T * 100, n = el('span', 'obs-tick', `${s} s`);
      n.style.left = pct + '%';
      n.style.transform = pct < 4 ? 'translate(0, -50%)' : pct > 92 ? 'translate(-100%, -50%)' : 'translate(-50%, -50%)';
      return n;
    }));
    $('#obsGrid').replaceChildren(...ticks.map(s => { const i = el('i'); i.style.left = (s / T * 100) + '%'; return i; }));
  }
  // the route line: the agents at the top level of the run, in the order they ran
  function obsRoute(run) {
    $('#obsQ').textContent = `“${run.req}”`;
    const hops = obsData.spans.filter(s => s.depth === 1).map(s => s.agent);
    const items = [];
    hops.forEach((a, k) => {
      if (k) items.push(el('li', 'obs-arrow', '→'));
      items.push(el('li', `a-${a}`, OBS_SHORT[a]));
    });
    $('#obsPath').replaceChildren(...items);
  }
  const obsTotals = (t, tok) => { $('#obsTime').textContent = t.toFixed(2) + ' s'; $('#obsTok').textContent = fmt(tok) + ' tokens'; };
  async function obsStream(key) {
    const me = ++obsRun;
    const run = OBS_RUNS.find(r => r.key === key);
    obsKey = key;
    obsData = run.build();
    obsData.total = obsData.spans[0].t1;
    obsApp.classList.toggle('failed', !!run.fail);
    $$('.obs-runchip', $('#obsRuns')).forEach(b => { const on = b.dataset.run === key; b.classList.toggle('is-on', on); b.setAttribute('aria-pressed', String(on)); });
    obsAxis();
    obsRoute(run);
    obsRows.replaceChildren();
    const { spans, total } = obsData;
    const tokens = upTo => spans.reduce((a, s) => a + (s.tok && s.t1 <= upTo ? s.tok : 0), 0);
    if (reduced || !isActive('observe')) {
      obsRows.replaceChildren(...spans.map((s, i) => obsRowEl(s, i, 0)));
      obsTotals(total, tokens(total));
      obsSelect(obsData.pick);
      return;
    }
    const STREAM = total > 8 ? 3600 : 2600;
    obsApp.classList.add('streaming');
    obsInsp.replaceChildren(el('p', 'obs-meta', 'Steps appear as the run executes. Click any step to inspect it.'));
    const t0 = performance.now();
    const tick = now => {
      if (me !== obsRun) return;
      const at = Math.min(total, (now - t0) / STREAM * total);
      obsTotals(at, tokens(at));
      if (at < total) requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
    for (let i = 0; i < spans.length; i++) {
      const wait = t0 + spans[i].t0 / total * STREAM - performance.now();
      if (wait > 0) await sleep(wait);
      if (me !== obsRun || !isActive('observe')) { if (me === obsRun) obsApp.classList.remove('streaming'); return; }
      obsRows.append(obsRowEl(spans[i], i, (spans[i].t1 - spans[i].t0) / total * STREAM));
    }
    await sleep(Math.max(0, t0 + STREAM - performance.now()) + 350);
    if (me !== obsRun) return;
    obsApp.classList.remove('streaming');
    obsTotals(total, tokens(total));
    obsSelect(obsData.pick);
  }
  $('#obsRuns').replaceChildren(...OBS_RUNS.map(r => {
    const b = el('button', 'obs-runchip');
    b.type = 'button';
    b.dataset.run = r.key;
    b.setAttribute('aria-pressed', 'false');
    b.setAttribute('aria-label', `${r.req}: ${r.tags.map(a => OBS_AGENT[a]).join(' then ')}${r.fail ? ', failed' : ''}`);
    b.append(el('span', null, r.label), ...r.tags.map(a => obsTag(a, OBS_SHORT[a])));
    if (r.fail) b.append(el('span', 'obs-atag fail', 'failed'));
    b.addEventListener('click', () => obsStream(r.key));
    return b;
  }));
  // sparklines (illustrative), and the internal cost per request from the cost slide's on-premises settings
  const OBS_SPARKS = {
    p95: [5.2, 5.0, 5.4, 4.9, 4.7, 5.1, 4.8, 4.6, 4.9, 5.3, 4.8, 4.7, 4.9, 4.6, 4.8, 5.0, 4.7, 4.6, 4.8, 4.9, 4.7, 4.8, 4.6, 4.8],
    tok: [2.9, 3.0, 3.1, 3.0, 3.2, 3.1, 3.0, 3.1, 3.2, 3.1, 3.0, 3.1, 3.1, 3.2, 3.0, 3.1, 3.1, 3.0, 3.2, 3.1, 3.1, 3.0, 3.1, 3.1],
    err: [0.6, 0.7, 0.5, 0.6, 0.8, 0.7, 0.6, 2.9, 0.9, 0.7, 0.6, 0.8, 0.7, 0.6, 0.7, 0.8, 0.6, 0.7, 0.8, 0.7, 0.6, 0.8, 0.7, 0.8],
  };
  $$('#observe .obs-spark').forEach(svg => {
    const v = OBS_SPARKS[svg.dataset.spark], lo = Math.min(...v), hi = Math.max(...v);
    const pts = v.map((y, i) => [i / (v.length - 1) * 120, 23 - (y - lo) / (hi - lo || 1) * 20]);
    svg.innerHTML = `<polyline points="${pts.map(p => p.map(n => n.toFixed(1)).join(',')).join(' ')}"/>`;
  });
  function obsCost() {
    const hw = +($('#cHw')?.value || 3000000), run = +($('#cRun')?.value || 125000);
    const perRequest = (hw / 36 + run) / 30 / 1284;     // monthly server cost spread over 1,284 requests a day
    $('#obsCost').textContent = `≈ ${inr(perRequest)}`;
  }
  $('#obsReplay').addEventListener('click', () => obsStream(obsKey));
  obsCost();
  obsStream('slow');
  hooks.observe = () => { obsCost(); obsStream(obsKey); };
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

  /* 5 work: the prompt splits into its six tokens one after another */
  $$('#work .frame-v[data-frame="1"] .tk-chip').forEach((c, k) => c.style.setProperty('--pz-k', k));

  /* 4 frozen: frost crystallises in the empty corner while the points freeze into place */
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
