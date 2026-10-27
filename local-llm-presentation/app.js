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
        if (!t.trim()) { frag.append(t); continue; }
        const s = el('span', 'tk', t);
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

  /* ------------------------------------------------------------------ 5: six steps of inference */
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

  /* ------------------------------------------------------------------ 8: cost calculator (rupees): on-premises or AWS */
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

  /* ------------------------------------------------------------------ 9: harness rings (layers light up in turn) */
  const RINGS = [['audit', 'Audit log'], ['guard', 'Rules'], ['perm', 'Access rights'], ['tools', 'Tools'], ['rag', 'Our documents']];
  function ringsSvg(onKeys) {
    const c = 180;
    let s = '';
    RINGS.forEach(([key, name], i) => {
      const r = 174 - i * 26;
      const on = onKeys.includes(key);
      s += `<circle class="ring${on ? ' on' : ''}" cx="${c}" cy="${c}" r="${r}"/>`;
      s += `<text class="ring-label${on ? ' on' : ''}" x="${c}" y="${c - r + 18}" text-anchor="middle">${name}</text>`;
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

  /* ------------------------------------------------------------------ 11: four routes */
  const PATHS = [
    { title: 'A · Use an open-weight model as it is',
      desc: 'Download a strong open model, run it on our own server and give people a private chat window. Everything stays in-house, but the model only knows what it learned in training. It knows nothing about our company.',
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
