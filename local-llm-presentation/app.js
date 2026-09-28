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
  function rangeInr(lo, hi, plus = '') {
    const [a, ua] = inrParts(lo), [b, ub] = inrParts(hi);
    const [c, uc] = usdParts(lo / INR_PER_USD), [d, ud] = usdParts(hi / INR_PER_USD);
    const i = ua === ub ? `₹${a}–${b}${ub}${plus}` : `₹${a}${ua} – ₹${b}${ub}${plus}`;
    const u = uc === ud ? `$${c}–${d}${ud}${plus}` : `$${c}${uc}–${d}${ud}${plus}`;
    return `${i} (${u})`;
  }
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
  function setBars(box, items, chosen = -1, targetIdx = -1) {
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
      r.classList.toggle('target', i === targetIdx);
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

  /* ------------------------------------------------------------------ 5: how large is large */
  const MODELS = [
    { name: 'GPT-2 · OpenAI, 2019', b: 1.5, mem: 'about 1 GB', runs: 'anything, even a phone',
      note: 'The model that first made people take machine-written text seriously. Tiny by today’s standards.' },
    { name: 'Small open model', b: 8, mem: 'about 5 GB', runs: 'a normal laptop',
      note: 'Llama 3.1 8B, Qwen3 8B and similar. Good for drafting, summaries and simple questions. Our prototype runs one at 91 tokens per second on a laptop.' },
    { name: 'Mid-size open model', b: 32, mem: 'about 20 GB', runs: 'a workstation with one graphics card',
      note: 'Qwen3 32B and similar. Strong at reasoning, writing and code. A sweet spot for a team server.' },
    { name: 'Large open model', b: 70, mem: 'about 40 GB', runs: 'a server with one or two data-centre GPUs',
      note: 'Llama 3.3 70B and similar. Handles most business writing and analysis well.' },
    { name: 'GPT-3 · OpenAI, 2020', b: 175, mem: 'about 100 GB', runs: 'a multi-GPU server',
      note: 'The model family that led to the first ChatGPT. Today’s 32B open models beat it on almost every test.' },
    { name: 'Largest open models', b: 671, active: 37, mem: 'about 400 GB', runs: 'a multi-GPU server',
      note: 'DeepSeek-V3 and R1. A “mixture of experts”: only 37B of its parameters (in green) work on each token, so it runs like a much smaller model.' },
    { name: 'Frontier subscription models', b: 2000, est: true, mem: 'not published', runs: 'the vendor’s data centres only',
      note: 'GPT-5, Claude, Gemini. Sizes are not published; outside estimates run from hundreds of billions to several trillion. The dots here are a guess.' },
  ];
  const DOT_B = 0.25;
  const MAXDOTS = 2000 / DOT_B;
  const canvas = $('#scaleCanvas');
  const ctx = canvas.getContext('2d');
  const range = $('#scaleRange');
  const cssVar = n => getComputedStyle(html).getPropertyValue(n).trim();
  const C = { dot: cssVar('--ink'), empty: cssVar('--line'), live: cssVar('--ours'), est: cssVar('--faint') };
  let grid = null, shown = MODELS[1].b / DOT_B, scaleModel = MODELS[1], scaleAnim = 0;

  function sizeCanvas() {
    const w = canvas.offsetWidth, h = canvas.offsetHeight;
    if (!w) return;
    const dpr = Math.min(3, (devicePixelRatio || 1) * S);
    canvas.width = Math.round(w * dpr);
    canvas.height = Math.round(h * dpr);
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    let step = Math.sqrt(w * h / MAXDOTS);
    const cols = Math.max(1, Math.floor(w / step));
    step = Math.min(w / cols, h / Math.ceil(MAXDOTS / cols));
    grid = { cols, step, size: Math.max(1, step * 0.6), w, h };
    drawScale();
  }
  const isActiveDot = i => ((i * 2654435761) >>> 0) % 1000 < 55;
  function drawScale() {
    if (!grid) return;
    const { cols, step, size, w, h } = grid;
    ctx.clearRect(0, 0, w, h);
    const lit = Math.round(shown);
    const at = i => [(i % cols) * step, Math.floor(i / cols) * step];
    ctx.fillStyle = C.empty;
    for (let i = lit; i < MAXDOTS; i++) { const [x, y] = at(i); ctx.fillRect(x, y, size, size); }
    ctx.fillStyle = scaleModel.est ? C.est : C.dot;
    for (let i = 0; i < lit; i++) {
      if (scaleModel.active && isActiveDot(i)) continue;
      const [x, y] = at(i); ctx.fillRect(x, y, size, size);
    }
    if (scaleModel.active) {
      ctx.fillStyle = C.live;
      for (let i = 0; i < lit; i++) if (isActiveDot(i)) { const [x, y] = at(i); ctx.fillRect(x, y, size, size); }
    }
  }
  function setScale(idx, animate = true) {
    const m = MODELS[idx];
    scaleModel = m;
    $('#scaleName').textContent = m.name;
    $('#scaleParams').textContent = m.est ? '1,000s?' : m.b.toLocaleString('en-US');
    $('#scaleMem').textContent = m.mem + (m.est ? '' : ' at 4-bit precision');
    $('#scaleRuns').textContent = m.runs;
    $('#scaleNote').textContent = m.note;
    $$('.scale-ticks li').forEach((li, i) => li.classList.toggle('on', i === idx));
    const target = m.b / DOT_B;
    cancelAnimationFrame(scaleAnim);
    if (!animate || reduced) { shown = target; drawScale(); return; }
    const from = shown, t0 = performance.now();
    const tick = now => {
      const k = clamp((now - t0) / 700);
      shown = from + (target - from) * (1 - Math.pow(1 - k, 3));
      drawScale();
      if (k < 1) scaleAnim = requestAnimationFrame(tick);
    };
    scaleAnim = requestAnimationFrame(tick);
  }
  $$('.scale-ticks li').forEach((li, i, all) => { li.style.left = (i / (all.length - 1) * 100) + '%'; });
  range.addEventListener('input', () => setScale(+range.value));
  setScale(1, false);
  hooks.scale = () => { sizeCanvas(); };

  /* ------------------------------------------------------------------ 7: train a tiny model */
  const WORDS = ['Paris', 'London', 'beautiful', 'a', 'Lyon'];
  const LR = 0.6;
  let logits, trainStep, losses, training = false;
  const trainBars = $('#trainBars');
  const trainMsg = $('#trainMsg');
  const lossSvg = $('#lossChart');
  const lossOf = () => -Math.log(softmax(logits)[0]);
  function renderTrain() {
    const p = softmax(logits);
    const top = p.indexOf(Math.max(...p));
    setBars(trainBars, WORDS.map((w, i) => ({ label: `"${w}"`, p: p[i] })), top, 0);
    const pc = Math.round(p[0] * 100);
    trainMsg.textContent =
      trainStep === 0 ? `Step 0. It has no idea yet: its top guess is "${WORDS[top]}".` :
      p[0] < 0.4 ? `Step ${trainStep}. Getting warmer. "Paris" is now at ${pc}%.` :
      p[0] < 0.9 ? `Step ${trainStep}. "Paris" is now its top guess, at ${pc}%.` :
      `Step ${trainStep}. Learned (${pc}%). Real training does this for trillions of words, adjusting billions of parameters at once.`;
    drawLoss();
  }
  function drawLoss() {
    const W = 420, H = 260, l = 40, r = 12, t = 14, b = 34;
    const n = Math.max(20, losses.length - 1), maxL = 2;
    const X = i => l + (i / n) * (W - l - r);
    const Y = v => t + (1 - clamp(v / maxL)) * (H - t - b);
    const pts = losses.map((v, i) => `${X(i).toFixed(1)},${Y(v).toFixed(1)}`).join(' ');
    const last = losses[losses.length - 1];
    lossSvg.innerHTML =
      [0.5, 1, 1.5, 2].map(v => `<line class="grid-line" x1="${l}" x2="${W - r}" y1="${Y(v)}" y2="${Y(v)}"/><text class="svg-label" x="${l - 8}" y="${Y(v) + 4}" text-anchor="end">${v}</text>`).join('') +
      `<line class="axis" x1="${l}" x2="${W - r}" y1="${Y(0)}" y2="${Y(0)}"/>` +
      `<text class="svg-label" x="${l - 8}" y="${Y(0) + 4}" text-anchor="end">0</text>` +
      `<text class="svg-label" x="${W - r}" y="${H - 10}" text-anchor="end">training steps → ${losses.length - 1}</text>` +
      `<polyline class="loss-line" points="${pts}"/>` +
      `<circle class="loss-dot" cx="${X(losses.length - 1)}" cy="${Y(last)}" r="5"/>`;
  }
  function trainOnce() {
    const p = softmax(logits);
    logits = logits.map((v, i) => v - LR * (p[i] - (i === 0 ? 1 : 0)));
    trainStep++;
    losses.push(lossOf());
  }
  async function train(n) {
    if (training) return;
    training = true;
    for (let k = 0; k < n; k++) { trainOnce(); renderTrain(); if (!reduced && n > 1) await sleep(70); }
    training = false;
  }
  function resetTrain() { logits = [0.1, -0.05, 0.35, 0.25, -0.2]; trainStep = 0; losses = [lossOf()]; renderTrain(); }
  $('#train1').addEventListener('click', () => train(1));
  $('#train20').addEventListener('click', () => train(20));
  $('#trainReset').addEventListener('click', () => { if (!training) resetTrain(); });
  resetTrain();

  /* ------------------------------------------------------------------ 9: six steps of inference */
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

  /* ------------------------------------------------------------------ 10: temperature */
  const TEMP = [['safe', 2.3], ['private', 2.0], ['secure', 1.7], ['protected', 1.4], ['yours', 0.9], ['moving', -0.2], ['guessing', -0.9], ['in a volcano', -2.6]];
  const tempRange = $('#tempRange');
  const tempProbs = () => softmax(TEMP.map(x => x[1]), +tempRange.value);
  function renderTemp() {
    const T = +tempRange.value;
    $('#tempVal').textContent = T.toFixed(1);
    const p = tempProbs();
    setBars($('#tempBars'), TEMP.map(([w], i) => ({ label: `"${w}"`, p: p[i] })), 0);
    $('#tempHint').textContent =
      T < 0.4 ? 'Almost always the same word. Right for facts, numbers and code.' :
      T <= 1.1 ? 'Mostly sensible, with some variety. The usual setting for chat.' :
      'Odd words creep in. Fine for brainstorming, risky for anything factual.';
  }
  function sampleTemp() {
    const p = tempProbs();
    const out = [];
    for (let k = 0; k < 12; k++) {
      let r = Math.random(), i = 0;
      while (i < p.length - 1 && (r -= p[i]) > 0) i++;
      const chip = el('span', 'sc' + (p[i] < 0.08 ? ' rare' : ''), TEMP[i][0]);
      chip.style.animationDelay = (k * 40) + 'ms';
      out.push(chip);
    }
    $('#tempSamples').replaceChildren(...out);
  }
  tempRange.addEventListener('input', renderTemp);
  $('#tempSample').addEventListener('click', sampleTemp);
  renderTemp();
  sampleTemp();

  /* ------------------------------------------------------------------ 11: tokenizer */
  const tokIn = $('#tokIn');
  function renderTok() {
    const text = tokIn.value;
    const toks = tokenize(text);
    const shownToks = toks.slice(0, 120);
    $('#tokOut').replaceChildren(...shownToks.map((t, i) => {
      const nl = t.includes('\n');
      const s = el('span', 'tc' + (nl ? ' nl' : ''), nl ? '↵' : t);
      s.style.background = HL[i % HL.length];
      return s;
    }));
    if (toks.length > shownToks.length) $('#tokOut').append(el('span', 'tc nl', ` … +${fmt(toks.length - shownToks.length)} more`));
    const words = (text.match(/\S+/g) || []).length;
    const stats = [
      ['Characters', fmt(text.length)], ['Words', fmt(words)], ['Tokens', fmt(toks.length)],
      ['Of a 32K context window', (toks.length / 32768 * 100).toFixed(toks.length > 3276 ? 0 : 2) + '%'],
    ];
    $('#tokStats').replaceChildren(...stats.map(([k, v]) => { const d = el('div'); d.append(el('dt', null, k), el('dd', null, v)); return d; }));
  }
  tokIn.addEventListener('input', renderTok);
  renderTok();

  /* ------------------------------------------------------------------ 12: glossary */
  const CATS = { basics: 'Basics', build: 'Building with AI', run: 'Running a model', risk: 'Risks and safety' };
  const TERMS = [
    ['Token', 'basics', 1, 'The unit a model reads and writes: a common word, or a piece of a rarer one. Usage limits and prices are counted in tokens.', 'Syllables, for a machine.', 'Tokenizers are learned from data, usually by byte-pair encoding. Vocabularies run from about 32,000 to over 200,000 tokens. English averages about 0.75 words per token; many other languages, and code, need more tokens for the same meaning.'],
    ['Parameters', 'basics', 1, 'The billions of numbers inside a model that were adjusted during training. Together they hold everything it learned.', 'The strength of the connections in a brain.', 'Also called weights. An 8B model has 8 billion of them. Memory needed ≈ parameters × bytes per parameter: 2 bytes at 16-bit, about 0.5 at 4-bit.'],
    ['Context window', 'basics', 1, 'How much text the model can consider at once: the instructions, the conversation so far and any documents supplied, measured in tokens.', 'A desk. Only what fits on the desk gets used.', 'Ranges from about 8,000 to over 1,000,000 tokens. Longer contexts need more memory (the KV cache), and models often miss facts buried in the middle of very long inputs, which is one reason RAG sends only the relevant passages.'],
    ['Prompt and system prompt', 'basics', 0, 'The prompt is what a person sends. The system prompt is standing instructions set by whoever runs the tool: role, rules, tone and format.', 'The briefing a new employee gets on day one, then the day’s requests.', 'In a harness, the system prompt also describes the available tools and how to call them. It is one of the most important levers we control; with a subscription service most of it belongs to the vendor.'],
    ['Inference', 'basics', 0, 'Using a trained model to produce an answer. It happens every time anyone asks a question.', 'Training is school. Inference is doing the job.', 'For a single user the cost is dominated by GPU memory and memory bandwidth rather than raw compute. Batching many users together is what makes a shared server efficient.'],
    ['Temperature', 'basics', 0, 'A setting that controls how adventurous the model is when it picks each next word. Low is predictable, high is creative.', 'A careful accountant versus a brainstorming session.', 'Probabilities are computed as softmax(scores ÷ temperature). Related settings, top-p and top-k, cut unlikely tokens out entirely.'],
    ['Knowledge cut-off', 'basics', 0, 'The date the training data ends. The model knows nothing after it unless the harness gives it documents or search results.', 'A well-read colleague who has been off-grid since that date.', 'Cut-offs are usually several months to a year before a model is released, and models are often unsure of their own cut-off.'],
    ['Multimodal', 'basics', 0, 'A model that handles more than text: images, scanned documents, charts, audio, sometimes video.', 'A colleague who reads the attachment, not only the email.', 'Usually a vision encoder turns an image into features the language model can read. Several open-weight families (Gemma, Qwen-VL, Llama 4) are multimodal.'],
    ['Reasoning model', 'basics', 0, 'A model trained to work through a problem step by step before answering. Slower, but much better at maths, code and planning.', 'Showing your working in an exam.', 'Examples: OpenAI’s o-series, DeepSeek-R1, Qwen3 in thinking mode. The thinking costs extra tokens, so answers take longer and use more compute.'],
    ['RAG', 'build', 1, 'Retrieval-augmented generation. Before answering, the system searches our documents for the relevant passages and gives them to the model with the question, so it answers from them and can cite them.', 'An open-book exam instead of answering from memory.', 'Split documents into passages, embed them, store them in an index; at question time search (vector plus keyword), rerank, and put the best passages into the prompt. Updating knowledge means updating the index: no retraining.'],
    ['Embeddings', 'build', 0, 'A way to turn a piece of text into a list of numbers that captures its meaning, so software can find passages that mean the same thing even when the words differ.', 'Map coordinates for meaning: similar ideas sit close together.', 'Made by a separate, small embedding model, typically 384 to 4,096 numbers per passage. Closeness is measured by the angle between two lists (cosine similarity).'],
    ['Vector database', 'build', 0, 'A database that stores embeddings and quickly finds the ones closest to a question. It is the search engine behind RAG.', 'A library catalogue organised by topic instead of title.', 'Options include PostgreSQL with pgvector, Qdrant, Milvus and OpenSearch. Store each passage’s access rights next to it, so search only returns what the asker may see.'],
    ['Fine-tuning', 'build', 1, 'Further training an existing model on our own examples, to change its behaviour, tone or output format.', 'On-the-job training for a graduate who already has a degree.', 'Good for style, format and narrow tasks such as sorting tickets. Poor at reliably adding facts, and whatever it learns is frozen on the day training ends. Needs hundreds to thousands of high-quality examples.'],
    ['LoRA', 'build', 0, 'A cheap way to fine-tune. Instead of changing every parameter, a small add-on is trained: hours on one GPU instead of weeks on many.', 'Clip-on lenses instead of new glasses.', 'Low-Rank Adaptation trains small extra matrices, typically well under 1% of the model’s size. QLoRA does the same on a 4-bit model to save memory. Different adapters can be swapped in per task on one base model.'],
    ['Harness', 'build', 1, 'All the software around a model that turns it into a useful tool: instructions, memory, access to documents and tools, permissions and logging.', 'The model is the engine; the harness is the rest of the car.', 'Examples: the ChatGPT app, Claude Code, and our own Local LLM PC Companion. The harness decides what goes into the context window at every step, so it drives quality as much as the model does.'],
    ['Agent', 'build', 0, 'A model inside a harness that works through a task in steps: search, read, calculate, write, check its own work, and repeat until done.', 'An assistant you give a goal, not a single instruction.', 'A loop of model call → tool call → result → model call. Agents need limits: approval for risky actions, detection of loops, and a clear definition of done.'],
    ['Tool calling', 'build', 0, 'The model’s ability to ask the harness to run something, such as a search, a database query or a calculation, and then use the result.', 'Asking a colleague to check the system while you draft the reply.', 'Also called function calling. The model writes a structured request that matches a tool’s description; the harness checks it and runs it. The model itself never executes anything.'],
    ['MCP', 'build', 0, 'Model Context Protocol: an open standard for connecting AI tools to data sources and apps, so a connector is built once and works with many models.', 'USB for AI: one plug shape for many devices.', 'Introduced by Anthropic in November 2024 and since adopted widely, including by OpenAI and Google. An MCP server exposes tools and data; the harness is the client.'],
    ['Open-weight model', 'run', 1, 'A model whose trained parameters are published for anyone to download and run on their own hardware.', 'Getting the recipe, not just the meal.', 'Examples: Llama (Meta), Qwen (Alibaba), Gemma (Google), Mistral, DeepSeek, gpt-oss (OpenAI). Not the same as open source: training data and code are usually not released, and licences range from fully permissive (Apache 2.0) to custom terms with conditions.'],
    ['Quantization', 'run', 0, 'Storing each parameter with fewer bits, for example 4 instead of 16, so the model needs about a quarter of the memory for a small loss in quality.', 'A high-quality MP3 of a studio recording.', 'Common formats: GGUF (llama.cpp), AWQ and GPTQ (GPU servers), FP8. 4-bit keeps most of the quality for everyday work; 2 or 3 bits start to hurt noticeably.'],
    ['GPU and VRAM', 'run', 0, 'The graphics processor does the maths; its own memory (VRAM) has to hold the model. VRAM size decides which models a machine can run.', 'The size of the workbench decides how big a job you can take on.', 'Consumer cards have 8 to 32 GB, data-centre cards 48 GB and up. A model that does not fit spills into system memory at a large speed cost. Serving software: llama.cpp for single machines, vLLM or SGLang for many users.'],
    ['Tokens per second', 'run', 0, 'How fast a model writes. People read at about 5 tokens per second; a good local setup writes 30 to 100 or more.', 'Words per minute, for a machine.', 'Two numbers matter: prompt processing (reading, often thousands of tokens per second) and generation (writing). Generation speed is limited mainly by memory bandwidth.'],
    ['Mixture of experts', 'run', 0, 'A model split into many specialist sub-networks, with only a few switched on for each token. Big-model knowledge at small-model speed.', 'A hospital full of specialists, where each patient sees only two or three.', 'DeepSeek-V3: 671B parameters, 37B active per token. Qwen3-30B-A3B: 30B total, 3B active. All experts still have to fit in memory.'],
    ['Distillation', 'run', 0, 'Training a smaller model to imitate a bigger one. Many of the best small models are made this way.', 'An apprentice learning from a master.', 'The student learns from the teacher’s outputs. DeepSeek, for example, released R1 distilled into several smaller Qwen and Llama models.'],
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

  /* ------------------------------------------------------------------ 14: follow the data */
  const ICONS = {
    laptop: '<rect x="4" y="5" width="16" height="11" rx="1.5"/><path d="M2 19h20"/>',
    shield: '<path d="M12 3l7 3v6c0 4.5-3 7.5-7 9-4-1.5-7-4.5-7-9V6z"/>',
    globe: '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.5 2.6 3.7 5.4 3.7 8.5s-1.2 5.9-3.7 8.5c-2.5-2.6-3.7-5.4-3.7-8.5S9.5 6.1 12 3.5z"/>',
    cloud: '<path d="M7 18h10a4 4 0 0 0 .6-7.95A6 6 0 0 0 6.2 9.4 4.3 4.3 0 0 0 7 18z"/>',
    server: '<rect x="4" y="4" width="16" height="7" rx="1.5"/><rect x="4" y="13" width="16" height="7" rx="1.5"/><path d="M8 7.5h.01M8 16.5h.01"/>',
    chip: '<rect x="7" y="7" width="10" height="10" rx="1.5"/><path d="M10 3v4M14 3v4M10 17v4M14 17v4M3 10h4M3 14h4M17 10h4M17 14h4"/>',
  };
  const ROUTES = {
    cloud: {
      title: 'Five stops. The last three are outside our control.',
      stops: [
        { icon: 'laptop', name: 'A colleague’s laptop', sub: 'Asks a question and attaches a contract' },
        { icon: 'shield', name: 'Our network', sub: 'Office network and firewall' },
        { icon: 'globe', name: 'The internet', sub: 'Encrypted in transit', flag: 'Leaves our network' },
        { icon: 'cloud', name: 'Vendor’s data centre', sub: 'Location and staff chosen by the vendor', flag: 'Their rules' },
        { icon: 'chip', name: 'Vendor’s model', sub: 'Shared with their other customers' },
      ],
      notes: ['The question and the contract leave our network', 'Stored under the vendor’s retention policy', 'Needs the internet to work', 'Vendor price changes and limits apply'],
    },
    local: {
      title: 'Four stops. Every one of them is ours.',
      stops: [
        { icon: 'laptop', name: 'A colleague’s laptop', sub: 'Asks a question and attaches a contract' },
        { icon: 'shield', name: 'Our network', sub: 'Office network and firewall' },
        { icon: 'server', name: 'Our AI server', sub: 'In our building, or our own cloud account', ours: 1 },
        { icon: 'chip', name: 'Our model', sub: 'Answers with our documents at hand', ours: 1 },
      ],
      notes: ['Nothing leaves our network', 'Stored, or not, under our own policy', 'Keeps working if the internet is down', 'No usage limits beyond our own hardware'],
    },
  };
  const routeEl = $('#route'), routeLine = $('#routeLine'), packet = $('#packet');
  let routeKey = 'cloud', centers = [], routeRaf = 0;
  function renderRoute() {
    const R = ROUTES[routeKey];
    $('#routeTitle').textContent = R.title;
    routeEl.replaceChildren(...R.stops.map(s => {
      const li = el('li', 'stop' + (s.ours ? ' ours' : ''));
      const node = el('span', 'node');
      node.innerHTML = `<svg viewBox="0 0 24 24" aria-hidden="true">${ICONS[s.icon]}</svg>`;
      li.append(node, el('span', 'stop-name', s.name), el('span', 'stop-sub', s.sub));
      if (s.flag) li.append(el('span', 'stop-flag', s.flag));
      return li;
    }));
    const notes = $('#routeNotes');
    notes.classList.toggle('ours', routeKey === 'local');
    notes.replaceChildren(...R.notes.map(n => el('li', null, n)));
    layoutRoute();
  }
  function layoutRoute() {
    const wrap = routeEl.parentElement.getBoundingClientRect();
    centers = $$('.node', routeEl).map(n => {
      const r = n.getBoundingClientRect();
      return { x: (r.left - wrap.left + r.width / 2) / S, y: (r.top - wrap.top + r.height / 2) / S };
    });
    if (centers.length < 2) return;
    const a = centers[0], b = centers[centers.length - 1];
    const vertical = Math.abs(b.y - a.y) > Math.abs(b.x - a.x);
    Object.assign(routeLine.style, vertical
      ? { left: (a.x - 1) + 'px', top: a.y + 'px', width: '2px', height: (b.y - a.y) + 'px' }
      : { left: a.x + 'px', top: (a.y - 1) + 'px', width: (b.x - a.x) + 'px', height: '2px' });
  }
  function animateRoute() {
    cancelAnimationFrame(routeRaf);
    const nodes = $$('.stop', routeEl);
    if (reduced || centers.length < 2) { packet.hidden = true; nodes.forEach(n => n.classList.add('lit')); return; }
    packet.hidden = false;
    const n = centers.length, SEG = 620, HOLD = 520;
    const leg = (n - 1) * SEG, cycle = 2 * leg + 2 * HOLD, t0 = performance.now();
    const frame = now => {
      if (!isActive('compare')) return;
      const t = Math.max(0, now - t0) % cycle;   // a frame's timestamp can be a little before t0
      let pos, back = false;
      if (t < leg) pos = t / SEG;
      else if (t < leg + HOLD) pos = n - 1;
      else if (t < 2 * leg + HOLD) { pos = (n - 1) - (t - leg - HOLD) / SEG; back = true; }
      else { pos = 0; back = true; }
      const i = Math.min(n - 2, Math.floor(pos)), k = pos - i;
      const x = centers[i].x + (centers[i + 1].x - centers[i].x) * k;
      const y = centers[i].y + (centers[i + 1].y - centers[i].y) * k;
      packet.style.transform = `translate(${x}px, ${y}px)`;
      packet.classList.toggle('back', back && routeKey === 'local');
      nodes.forEach((nd, j) => nd.classList.toggle('lit', Math.abs(pos - j) < 0.35));
      routeRaf = requestAnimationFrame(frame);
    };
    routeRaf = requestAnimationFrame(frame);
  }
  $$('.route-block .seg-btn').forEach(b => b.addEventListener('click', () => {
    routeKey = b.dataset.route;
    $$('.route-block .seg-btn').forEach(x => x.classList.toggle('is-on', x === b));
    renderRoute();
    animateRoute();
  }));
  renderRoute();
  hooks.compare = () => { layoutRoute(); animateRoute(); };

  /* ------------------------------------------------------------------ 17: cost calculator (rupees) */
  const cIn = { seats: $('#cSeats'), price: $('#cPrice'), hw: $('#cHw'), run: $('#cRun') };
  const calcSvg = $('#calcChart');
  const moneyHtml = r => `${inr(r)} <small>(${usd(r)})</small>`;
  const niceStep = v => { const p = Math.pow(10, Math.floor(Math.log10(v))); const m = v / p; return (m <= 1 ? 1 : m <= 2 ? 2 : m <= 5 ? 5 : 10) * p; };
  function renderCalc() {
    const seats = +cIn.seats.value, price = +cIn.price.value, hw = +cIn.hw.value, run = +cIn.run.value;
    $('#cSeatsOut').textContent = fmt(seats) + ' people';
    $('#cPriceOut').innerHTML = moneyHtml(price);
    $('#cHwOut').innerHTML = moneyHtml(hw);
    $('#cRunOut').innerHTML = moneyHtml(run);
    const cloudM = seats * price, M = 36;
    const cloud36 = cloudM * M, local36 = hw + run * M;
    const be = cloudM > run ? hw / (cloudM - run) : Infinity;
    $('#cCloud3').innerHTML = moneyHtml(cloud36);
    $('#cLocal3').innerHTML = moneyHtml(local36);
    $('#cBreak').innerHTML = be <= M ? `Month ${Math.max(1, Math.ceil(be))}` : be === Infinity ? 'Never, at these numbers' : 'After year 3';
    $('#cSave').innerHTML = cloud36 >= local36
      ? `${inr(cloud36 - local36)} saved <small>(${usd(cloud36 - local36)})</small>`
      : `${inr(local36 - cloud36)} more <small>(${usd(local36 - cloud36)})</small>`;

    const W = 760, H = 380, l = 92, r = 16, t = 14, b = 34;
    const top = Math.max(cloud36, local36) * 1.04;
    const step = niceStep(top / 4);
    const yMax = step * Math.ceil(top / step);
    const X = m => l + (m / M) * (W - l - r);
    const Y = v => t + (1 - v / yMax) * (H - t - b);
    let s = '';
    for (let v = 0; v <= yMax + 1; v += step) {
      s += `<line class="grid-line" x1="${l}" x2="${W - r}" y1="${Y(v)}" y2="${Y(v)}"/>`;
      s += `<text class="svg-label" x="${l - 10}" y="${Y(v) + 4}" text-anchor="end">${v ? inr(v) : '₹0'}</text>`;
    }
    for (const m of [0, 12, 24, 36]) {
      s += `<text class="svg-label" x="${X(m)}" y="${H - 10}" text-anchor="${m === 0 ? 'start' : m === 36 ? 'end' : 'middle'}">${m === 0 ? 'Today' : 'Year ' + m / 12}</text>`;
    }
    if (be < M) s += `<polygon class="area-save" points="${X(be)},${Y(hw + run * be)} ${X(M)},${Y(cloud36)} ${X(M)},${Y(local36)}"/>`;
    s += `<line class="line-cloud" x1="${X(0)}" y1="${Y(0)}" x2="${X(M)}" y2="${Y(cloud36)}"/>`;
    s += `<line class="line-local" x1="${X(0)}" y1="${Y(hw)}" x2="${X(M)}" y2="${Y(local36)}"/>`;
    if (be <= M) {
      const bx = X(be), by = Y(hw + run * be), anchor = bx > W - 240 ? 'end' : 'start';
      s += `<circle class="be-dot" cx="${bx}" cy="${by}" r="6"/>`;
      s += `<text class="be-label" x="${bx + (anchor === 'start' ? 12 : -12)}" y="${by - 12}" text-anchor="${anchor}">Pays for itself · month ${Math.max(1, Math.ceil(be))}</text>`;
    }
    calcSvg.setAttribute('viewBox', `0 0 ${W} ${H}`);
    calcSvg.innerHTML = s;
  }
  Object.values(cIn).forEach(i => i.addEventListener('input', renderCalc));
  renderCalc();

  /* ------------------------------------------------------------------ 19–20: myths */
  const MYTHS = [
    ['It understands and thinks like a person.', 'It predicts likely text extremely well, which can look like understanding, and it can reason through problems usefully. But it has no intentions, no awareness, and no memory of you beyond what the software gives it.'],
    ['It knows everything, up to today.', 'Its knowledge stops at a training cut-off, and it has never seen anything private, like our documents. That gap is exactly what RAG fills: the harness hands it the right documents when a question is asked.'],
    ['If it sounds confident, it is correct.', 'Fluency and accuracy are separate things. Models can invent figures, quotes and even sources in a perfectly confident tone. Answers built from our documents, with the sources shown, are far easier to trust and check.'],
    ['It learns from every conversation.', 'The model is frozen after training. Chat apps may remember things through a separate memory feature, and some plans let vendors train future models on chats. A model we host learns nothing unless we deliberately retrain it.'],
    ['Local models are toys compared to ChatGPT.', 'Open models have closed most of the gap. DeepSeek-R1 (January 2025) reported results comparable to OpenAI’s o1 on maths and coding, and OpenAI itself released open-weight models in August 2025. The best paid models still lead on the hardest problems.'],
    ['You need a data centre to run one.', 'An 8-billion-parameter model runs on an ordinary laptop, and one well-equipped server can serve a department. Our own prototype runs on a laptop with a 12 GB graphics card, and even on the processor alone.'],
    ['To teach it our business, we must train our own model.', 'Almost never. Giving it our documents at question time (RAG) is far cheaper, always up to date, and shows its sources. Fine-tuning changes style and format; training from scratch costs tens of crores of rupees or more.'],
    ['A bigger model always gives better answers.', 'For questions about our own business, having the right documents matters more than raw size. A mid-size model with good search often beats a giant one without it, and runs faster and cheaper.'],
    ['Open-weight means open source, with no strings attached.', 'The weights are downloadable, but the training data usually is not, and licences differ. Some are fully permissive (Apache 2.0); Meta’s Llama licence adds an acceptable-use policy and conditions for very large companies. Legal should review ours.'],
    ['Once it is installed, the job is done.', 'Like any system it needs an owner: testing, moving to better models as they appear, monitoring and security updates. It is a modest ongoing cost, and it belongs in the plan from day one.'],
  ];
  const checked = new Set();
  const updateMythCount = () => $$('.myth-num').forEach(n => { n.textContent = checked.size; });
  $$('.myth-grid').forEach(gridEl => {
    const from = +gridEl.dataset.myths;
    gridEl.replaceChildren(...MYTHS.slice(from, from + 5).map(([myth, real], k) => {
      const i = from + k;
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
  });
  updateMythCount();

  /* ------------------------------------------------------------------ 22: departments */
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

  /* ------------------------------------------------------------------ 23: four routes */
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

  /* ------------------------------------------------------------------ 25: harness rings (layers light up in turn) */
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

  /* ------------------------------------------------------------------ 26: harness builder */
  const H = { rag: $('#hRag'), tools: $('#hTools'), perm: $('#hPerm'), guard: $('#hGuard'), audit: $('#hAudit'), model: $('#hModel') };
  const MODEL_NAMES = ['Qwen3 32B', 'Llama 3.3 70B', 'gpt-oss-120b'];
  const hAnswer = $('#hAnswer');
  const cite = t => `<span class="cite">${t}</span>`;
  let firstHarness = true;
  function renderHarness() {
    const on = k => H[k].checked;
    const model = MODEL_NAMES[+H.model.value];
    const hasData = on('rag') || on('tools');
    const leak = hasData && !on('perm');
    let leave;
    if (on('rag') && on('tools')) leave = `You get 24 days of annual leave under our HR policy${cite('HR Policy 2026 §4.2')}, and the HR system shows you have used 9, so <b>15 days are left</b>.${cite('HR system · live')}`;
    else if (on('rag')) leave = `Our HR policy gives you 24 days of annual leave a year.${cite('HR Policy 2026 §4.2')} I can’t see how many you have already used.`;
    else if (on('tools')) leave = `The HR system shows <b>15 days left</b> this year.${cite('HR system · live')}`;
    else if (on('guard')) leave = `<span class="blocked">I can’t find your leave allowance in any source I have, so I won’t guess.</span>`;
    else leave = `<span class="guess">Most companies give 20 to 25 days of annual leave, so you probably have about 10 left.</span>`;
    let salary;
    if (leak) salary = `<span class="leak">Your manager, Priya, earns ${both(3200000)} a year${cite('payroll-2026.xlsx')}. A Sales employee should never see this.</span>`;
    else if (hasData) salary = `<span class="blocked">Salary details are restricted to HR, so I can’t share them.</span>`;
    else if (on('guard')) salary = `<span class="blocked">I have no source for salary information.</span>`;
    else salary = `<span class="guess">A sales manager typically earns ${rangeInr(2000000, 3500000)} a year.</span>`;
    const paint = () => { hAnswer.innerHTML = `<p>${leave}</p><p>${salary}</p>`; hAnswer.classList.remove('swap'); };
    if (firstHarness || reduced) paint();
    else { hAnswer.classList.add('swap'); setTimeout(paint, 160); }
    firstHarness = false;

    const guessed = !hasData && !on('guard');
    $('#hMeta').innerHTML = `Answered by <b>${model}</b> · ${guessed ? 'from memory: <b>guessed</b>' : hasData ? 'from our sources' : 'declined to guess'}`;
    const log = $('#hLog');
    log.hidden = !on('audit');
    if (on('audit')) {
      log.innerHTML = [
        '<b>10:42:07</b> sam.k (Sales) asked about leave and a manager’s salary',
        on('rag') ? '&nbsp;&nbsp;retrieved: HR Policy 2026 §4.2' : null,
        on('tools') ? '&nbsp;&nbsp;tool: hr.leave_balance(sam.k) → 15 days' : null,
        hasData ? (leak ? '&nbsp;&nbsp;<span class="alert">ALERT payroll data shown to a non-HR user</span>' : '&nbsp;&nbsp;salary request blocked: not in HR') : null,
        `&nbsp;&nbsp;model: ${model}`,
      ].filter(Boolean).join('<br>');
    }
    const n = RINGS.filter(([k]) => on(k)).length;
    $('#hScore').innerHTML = `<b>${n} of 5</b> layers on. ` + (
      leak && on('guard') ? 'Rules in a prompt can’t stop a leak. Access rights can.' :
      leak ? 'Documents are connected, but nothing checks who is asking. Turn on access rights.' :
      n === 0 ? 'No harness: the bare model guesses.' :
      n === 5 ? 'Every decision here is ours. Try another model: the answer stays grounded.' :
      'Keep going.');
  }
  ['rag', 'tools', 'perm', 'guard', 'audit', 'model'].forEach(k => H[k].addEventListener('change', renderHarness));
  renderHarness();

  /* ------------------------------------------------------------------ start */
  function relayout() {
    const modeChanged = fit();
    sizeCanvas();
    drawAttention();
    layoutRoute();
    if (modeChanged) {
      slides.forEach(s => s.classList.remove('on'));
      visible.clear();
      if (isDeck()) go(cur, true); else slides[cur].scrollIntoView();
    }
  }
  let resizeT = 0;
  addEventListener('resize', () => { clearTimeout(resizeT); resizeT = setTimeout(relayout, 120); });
  document.fonts?.ready.then(() => { sizeCanvas(); drawAttention(); layoutRoute(); });

  fit();
  sizeCanvas();
  const startId = location.hash.slice(1);
  cur = Math.max(0, slides.findIndex(s => s.id === startId));
  if (isDeck()) go(cur, true);
  else if (cur) slides[cur].scrollIntoView();
  updateBar();
})();
