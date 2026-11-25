// agentMon: renders the hive. Two inputs: board-data.js (the data script, every minute: projects, git,
// PRs, titles) and, in the desktop app, live snapshots pushed every second (open sessions, the work under
// them, the kitten). Views: #home, #sessions, #projects[/name], #prs. The page is patched in place, so
// running animations keep going; view switches cross-fade.
(function () {
  'use strict';
  const $ = (id) => document.getElementById(id);
  const view = $('view');
  const app = !!window.AGENTMON_APP && !!(window.ipc && window.ipc.postMessage);
  const post = (m) => { if (app) window.ipc.postMessage(m); };
  let slow = null, slowStamp = '', live = null, shownView = '';

  // ---------- helpers ----------
  const esc = (s) => String(s == null ? '' : s).replace(/[&<>"']/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;' }[c]));
  const ms = (iso) => (iso ? Date.now() - new Date(iso).getTime() : Infinity);
  function ago(iso) {
    const s = ms(iso) / 1000;
    if (!isFinite(s)) return '';
    if (s < 45) return 'just now';
    if (s < 3600) return Math.round(s / 60) + ' min ago';
    if (s < 86400) return Math.round(s / 3600) + ' h ago';
    return Math.round(s / 86400) + ' d ago';
  }
  function dur(iso) {
    const m = Math.max(0, Math.round(ms(iso) / 60000));
    return m < 1 ? 'just started' : m < 60 ? m + ' min' : Math.floor(m / 60) + ' h ' + (m % 60) + ' min';
  }
  // Warm hues only (honey, amber, rose, leaf, sky) so every project still reads as part of the hive.
  const HUES = [38, 18, 52, 345, 95, 200, 28, 8, 140, 265];
  function hue(name) { let h = 0; for (const c of name) h = (h * 31 + c.charCodeAt(0)) >>> 0; return HUES[h % HUES.length]; }
  function initials(name) {
    const w = name.replace(/^hails[.\-]?/i, '').split(/[\s._-]+/).filter(Boolean);
    const p = w.length ? w : [name];
    return ((p[0][0] || '') + (p[1] ? p[1][0] : (p[0][1] || ''))).toUpperCase();
  }
  const av = (name, size) => `<span class="av ${size || ''}" style="--h:${hue(name)}" aria-hidden="true">${esc(initials(name))}</span>`;
  const tag = (cls, text) => `<span class="tag ${cls}"><i></i>${esc(text)}</span>`;
  const sessName = (s) => s.name || s.title || s.aiTitle || s.id.slice(0, 8);
  const projHref = (p) => '#projects/' + encodeURIComponent(p.name);
  function tokens(s) { return String(s || '').toLowerCase().split(/[\s._\-\/\\]+/).filter((t) => t.length >= 3); }

  // ---------- individuality: every bee and cell gets its own stable traits ----------
  function seedOf(str) { let h = 2166136261; for (const c of String(str)) { h ^= c.charCodeAt(0); h = Math.imul(h, 16777619); } return h >>> 0; }
  function rng(seed) { let s = seed || 1; return () => { s ^= s << 13; s ^= s >>> 17; s ^= s << 5; return ((s >>> 0) % 1000003) / 1000003; }; }
  const BODIES = ['#f5b83d', '#efa21d', '#ffd24a', '#e8952a', '#ffcf6b', '#d98a14', '#f7c14f'];
  const traitCache = new Map();
  function traits(id) {
    let t = traitCache.get(id);
    if (t) return t;
    const r = rng(seedOf(id));
    t = {
      scale: 0.8 + r() * 0.4, body: BODIES[Math.floor(r() * BODIES.length)], stripes: r() < 0.35 ? 3 : 2, stripeW: 2.1 + r() * 1.4,
      plump: 0.88 + r() * 0.3, head: 3.3 + r() * 1.5, wing: 0.78 + r() * 0.5, wingTilt: r() * 16 - 8, antenna: r() * 8 - 4,
      flapA: 0.055 + r() * 0.07, flapB: 0.055 + r() * 0.07, buzz: 2.1 + r() * 3.4, buzzKind: Math.floor(r() * 4), buzzDelay: -r() * 6,
      jitter: 0.22 + r() * 0.3, tilt: r() * 26 - 13, build: 11 + r() * 17, buildKind: Math.floor(r() * 3), buildDelay: -r() * 20,
      fill: 7 + r() * 11, fillDelay: -r() * 30, fillKind: Math.floor(r() * 3), wave: 1.4 + r() * 2.6, waveBack: r() < 0.4, wobble: 1.6 + r() * 1.6, seed: seedOf(id), fx1: 0.6 + r() * 1.2, fx2: 1.4 + r() * 1.6, fy1: 0.7 + r() * 1.3, fy2: 1.6 + r() * 1.9, slosh1: 0.5 + r() * 0.9, slosh2: 1.1 + r() * 1.3, waveSpeed: 1.2 + r() * 2.4, buildA: 0.12 + r() * 0.25, buildB: 0.31 + r() * 0.4,
    };
    traitCache.set(id, t);
    return t;
  }
  const f2 = (n) => n.toFixed(2);
  // One bee drawn from its traits (centered on 0,0, about 24 units long before scaling).
  function beeInner(t) {
    const rx = 9 * t.plump, ry = 6;
    let stripes = '';
    const xs = t.stripes === 3 ? [-4.6, -0.8, 3] : [-3.6, 1.4];
    for (const x of xs) stripes += `<path d="M${x} ${-ry + 0.6}v${(ry - 0.6) * 2}" stroke="#2b1a00" stroke-width="${f2(t.stripeW)}"/>`;
    const w = t.wing;
    return `<g transform="rotate(${f2(t.tilt)}) scale(${f2(t.scale)})">
      <ellipse class="wing" cx="-2" cy="${f2(-5.5 * w)}" rx="${f2(4 * w)}" ry="${f2(6 * w)}" transform="rotate(${f2(-8 + t.wingTilt)} -2 ${f2(-5.5 * w)})" style="animation-duration:${f2(t.flapA)}s"/>
      <ellipse class="wing" cx="3" cy="${f2(-5.8 * w)}" rx="${f2(3.8 * w)}" ry="${f2(5.6 * w)}" transform="rotate(${f2(10 + t.wingTilt)} 3 ${f2(-5.8 * w)})" style="animation-duration:${f2(t.flapB)}s;animation-delay:-${f2(t.flapA / 2)}s"/>
      <ellipse cx="0" cy="1" rx="${f2(rx)}" ry="${ry}" fill="${t.body}" stroke="#2b1a00" stroke-width="1"/>
      <g clip-path="none">${stripes}</g>
      <circle cx="${f2(rx + t.head * 0.55)}" cy="0" r="${f2(t.head)}" fill="#2b1a00"/>
      <circle cx="${f2(rx + t.head * 0.9)}" cy="-1" r="0.9" fill="#fff"/>
      <path d="M${f2(rx + t.head * 0.4)} ${f2(-t.head * 0.8)}q2 -4 ${f2(4 + t.antenna * 0.4)} ${f2(-4.5 + t.antenna * 0.3)}" fill="none" stroke="#2b1a00" stroke-width="1" stroke-linecap="round"/>
      <path d="M${f2(-rx)} 1l-3 0.6" stroke="#2b1a00" stroke-width="1.4" stroke-linecap="round"/></g>`;
  }
  // A bee that buzzes about in its own way: its own path, pace and phase. The bee is its own small layer:
  // the path and the jitter move the layer on the GPU (no redraw); only the tiny wings redraw as they flap.
  function beeLayer(t, extra) {
    return `<div class="beebox${extra ? ' ' + extra : ''}"><div class="bee buzz-${t.buzzKind}" style="animation-duration:${f2(t.buzz)}s;animation-delay:${f2(t.buzzDelay)}s">`
      + `<div class="bee-jit" style="animation-duration:${f2(t.jitter)}s"><svg class="bee-svg" viewBox="-15 -14 32 26" aria-hidden="true">${beeInner(t)}</svg></div></div></div>`;
  }
  const miniBee = (id) => `<svg class="mini-bee" viewBox="-14 -13 30 24" aria-hidden="true">${beeInner(traits(id))}</svg>`;

  // ---------- session state ----------
  // busy: thinking; shell: running a command; idle + child processes: background work;
  // waiting: asking you something (permission or a question); idle: done, waiting on your reply.
  function sstate(s) {
    if (!s.running) return { key: 'closed', tone: '', label: 'closed' };
    const t = s.tasks || [];
    if (s.status === 'waiting') return { key: 'asks', tone: 'bad', label: 'needs your answer' };
    if (s.queued) return { key: 'busy', tone: 'warn', label: `waiting for a build slot (${ordinal(s.queued)} in line)` };
    if (s.pausedJob) return { key: 'busy', tone: 'warn', label: 'build paused, machine full' };
    if (s.status === 'busy') return { key: 'busy', tone: 'busy', label: 'working' };
    if (s.status === 'shell') return { key: 'busy', tone: 'busy', label: t.length ? 'running ' + t[t.length - 1].label : 'running a command' };
    if (t.length) return { key: 'busy', tone: 'busy', label: 'background: ' + t[t.length - 1].label };
    if (s.status === 'idle') return { key: 'ready', tone: 'ready', label: 'ready for you' };
    return { key: 'ready', tone: 'ready', label: s.status || 'open' };
  }
  function kittenTone(mood, text) {
    // "Question: ..." = an agent waiting on your answer.
    if (/question|ask/i.test(mood || '') || /^Question\b/i.test(text || '')) return 'question';
    if (/fail|problem|error|worried/i.test(mood || '')) return 'problem';
    if (/wait/i.test(mood || '')) return 'waiting';
    if (mood) return 'work';
    return /^(Failed|Error|Problem|Blocked)\b/.test(text) ? 'problem' : /^Waiting\b/.test(text) ? 'waiting' : 'work';
  }

  // ---------- live merge: the slow data plus the latest live snapshot ----------
  const askedFor = new Set();
  const norm = (p) => String(p || '').toLowerCase().replace(/\//g, '\\').replace(/\\+$/, '');
  function merged() {
    if (!slow) return null;
    const d = structuredClone(slow);
    if (!live) return d;
    const L = live.data, t = live.t;
    const owners = [];
    for (const p of d.projects) {
      owners.push([norm(p.root), p]);
      for (const w of (p.git ? p.git.worktrees : [])) owners.push([norm(w.path), p]);
    }
    owners.sort((a, b) => b[0].length - a[0].length);
    const find = (cwd) => { const c = norm(cwd); for (const [r, p] of owners) if (c === r || c.startsWith(r + '\\')) return p; return null; };
    const liveIds = new Set(L.sessions.map((s) => s.id));
    for (const p of d.projects) for (const s of p.sessions) if (s.running && !liveIds.has(s.id)) { s.running = false; s.status = ''; s.tasks = []; }
    for (const ls of L.sessions) {
      const p = find(ls.cwd);
      if (!p) { if (!askedFor.has(ls.id)) { askedFor.add(ls.id); post('refresh'); } continue; }
      let s = p.sessions.find((x) => x.id === ls.id);
      const labels = new Map(((s && s.tasks) || []).map((x) => [x.pid, x.label]));
      const tasks = (ls.tasks || []).map((x) => ({ pid: x.pid, label: x.label || labels.get(x.pid) || x.exe, started: x.started ? new Date(x.started).toISOString() : null, cpuAt: x.cpuAt || null }));
      if (!s) {
        s = { id: ls.id, name: '', title: '', aiTitle: '', where: '', lastWrite: new Date(t).toISOString(), lastPrompt: '', lastSaid: '' };
        p.sessions.unshift(s);
      }
      Object.assign(s, { running: true, status: ls.status || '', name: ls.name || s.name, pid: ls.pid, tasks, since: ls.since || null, activity: ls.activity || null, subagents: ls.subagents || [] });
      if (norm(ls.cwd) !== norm(p.root)) s.where = String(ls.cwd).split(/[\\/]/).pop();
    }
    if (Array.isArray(L.kitten)) {
      d.kitten = L.kitten.map((k) => ({
        name: k.session || '', text: k.text, time: new Date(t - (k.age_secs || 0) * 1000).toISOString(), mood: kittenTone(k.mood, k.text),
        kind: k.mood || '', progress: k.progress == null ? null : k.progress, progressLabel: k.progress_label || '', project: '',
      }));
    }
    d.gate = L.gate || null;
    d.agents = L.agents || [];
    d.jobs = L.jobs || [];
    return d;
  }
  function ordinal(n) { const s = ['th', 'st', 'nd', 'rd'], v = n % 100; return n + (s[(v - 20) % 10] || s[v] || s[0]); }

  // ---------- derived ----------
  function kittenProject(k, projects) {
    if (k.project) return projects.find((p) => p.name === k.project) || null;
    if (!k.name) return null;
    const kt = tokens(k.name);
    for (const p of projects) {
      if (p.sessions.some((s) => s.name && s.name.toLowerCase() === k.name.toLowerCase())) return p;
      const leaves = (p.git ? p.git.worktrees : []).map((w) => w.leaf.toLowerCase());
      if (kt.some((t) => leaves.includes(t))) return p;
      const pt = tokens(p.name).filter((t) => t !== 'hails');
      if (kt.some((k2) => k2.length >= 4 && pt.some((t) => t.startsWith(k2)))) return p;
    }
    return null;
  }
  function derive(d) {
    const projects = d.projects.slice();
    const sessions = [];
    // The build gate: who is queued (and where), whose build agentMon paused.
    const gate = d.gate || null;
    const queuedAt = new Map(), pausedFor = new Map();
    if (gate) {
      gate.waiting.forEach((w, i) => { if (w.sid && !queuedAt.has(w.sid)) queuedAt.set(w.sid, i + 1); });
      for (const j of gate.jobs) if (j.paused && j.sid) pausedFor.set(j.sid, j);
    }
    for (const p of projects) for (const s of p.sessions) {
      const x = Object.assign({ project: p }, s);
      x.queued = s.running ? queuedAt.get(s.id) || 0 : 0;
      x.pausedJob = s.running ? pausedFor.get(s.id) || null : null;
      x.st = sstate(x);
      sessions.push(x);
    }
    const running = sessions.filter((s) => s.running);
    const by = (k) => running.filter((s) => s.st.key === k);
    const kitten = d.kitten.map((k) => Object.assign({ proj: kittenProject(k, projects) }, k));
    const needs = [];
    for (const k of kitten) if (k.mood !== 'work') needs.push({ tone: k.mood === 'problem' ? 'bad' : 'warn', title: k.text, sub: (k.mood === 'question' ? 'Asking you · ' : 'Kitten') + (k.name ? (k.mood === 'question' ? '' : ': ') + k.name : ''), when: k.time, proj: k.proj, key: 'k' + k.name + k.text });
    // Background jobs that are blocked on you.
    for (const j of d.jobs || []) if (j.needs) needs.push({ tone: 'warn', title: `Job needs you: ${j.needs}`, sub: j.detail || j.state, when: j.updated, key: 'job' + j.id });
    for (const p of projects) {
      for (const pr of p.prs) if (pr.checks === 'fail') needs.push({ tone: 'bad', title: `#${pr.number} checks failing`, sub: pr.title, url: pr.url, proj: p, key: 'pr' + pr.url });
      for (const r of p.runs) if (r.result === 'fail') needs.push({ tone: 'bad', title: `CI ${r.state}: ${r.workflow}`, sub: `${r.repo} on ${r.branch}`, url: r.url, when: r.created, proj: p, key: 'run' + r.url });
      if (p.git && p.git.behind) needs.push({ tone: 'warn', title: `${p.git.behind} commits behind ${p.git.upstream}`, sub: 'Pull before you start', proj: p, key: 'behind' + p.name });
    }
    for (const p of projects) {
      const mine = running.filter((s) => s.project === p);
      p._busy = mine.filter((s) => s.st.key === 'busy').length;
      p._asks = mine.filter((s) => s.st.key === 'asks').length;
      p._open = mine.length;
      p._dirty = p.git ? p.git.changed + p.git.worktrees.reduce((a, w) => a + w.changed, 0) : 0;
    }
    projects.sort((a, b) => b._asks - a._asks || b._busy - a._busy || b._open - a._open || ms(a.lastActive) - ms(b.lastActive));
    return { projects, sessions, running, busy: by('busy'), ready: by('ready'), asks: by('asks'), kitten, needs, gate, agents: d.agents || [], jobs: d.jobs || [] };
  }
  const kittenFor = (st, p) => st.kitten.filter((k) => k.proj === p);
  // The kitten line of one session: the line whose name shares a word with the session's name or worktree
  // (project words and generic words do not count). Without such a word, only a project with one working
  // session and one line gets that line; otherwise it stays on the project.
  const GENERIC = new Set(['agent', 'desktop', 'alpha', 'session', 'claude', 'main']);
  function sessionKitten(st, s) {
    const ks = kittenFor(st, s.project);
    if (!ks.length) return null;
    const own = new Set(tokens(s.project.name));
    const words = tokens([sessName(s), s.where || ''].join(' ')).filter((t) => t.length >= 4 && !own.has(t) && !GENERIC.has(t));
    const hit = ks.find((k) => tokens(k.name).filter((t) => !own.has(t)).some((t) => words.some((w) => t.startsWith(w) || (w.startsWith(t) && t.length >= 4))));
    if (hit) return hit;
    const busyHere = st.busy.filter((x) => x.project === s.project);
    return busyHere.length === 1 && ks.length === 1 ? ks[0] : null;
  }
  // The kitten's own progress, with an ETA from its pace so far (the time since this text went up).
  function kbar(k) {
    if (k.progress == null) return '';
    let eta = '';
    const el = (Date.now() - Date.parse(k.time)) / 1000;
    if (k.progress > 0.03 && k.progress < 0.98 && el > 20) eta = ' · about ' + fmt(el * (1 - k.progress) / k.progress) + ' left';
    return `<div class="kbar"><div class="meter"><span style="width:${(k.progress * 100).toFixed(1)}%"></span></div><span>${esc((k.progressLabel || Math.round(k.progress * 100) + '%') + eta)}</span></div>`;
  }

  // ---------- transitions: what just changed ----------
  const lastKey = new Map();        // session id -> state key at the last render
  const justUntil = new Map();      // session id -> time its "just changed" glow ends
  let nextExpiry = 0;
  function noteTransitions(st) {
    const now = Date.now();
    for (const s of st.running) {
      const prev = lastKey.get(s.id);
      if (prev && prev !== s.st.key) {
        justUntil.set(s.id, now + 4200);
        if ((s.st.key === 'ready' || s.st.key === 'asks') && prev === 'busy' && !document.hasFocus()) post('attention');
      }
      lastKey.set(s.id, s.st.key);
    }
    for (const id of [...lastKey.keys()]) if (!st.running.some((s) => s.id === id)) lastKey.delete(id);
    for (const [id, t] of justUntil) if (t < now) justUntil.delete(id);
    nextExpiry = Math.min(...[...justUntil.values()], Infinity);
  }
  const isJust = (id) => (justUntil.get(id) || 0) > Date.now();

  // ---------- time: how long things usually take, and whether something looks stuck ----------
  // History (kept in the widget's own storage): the durations of finished tasks per project + task, and of
  // finished working turns per project; the last 12 of each. Typical = the median; slow = the 80th percentile.
  const TIMES_KEY = 'am-times-v1';
  let times;
  try { times = JSON.parse(localStorage.getItem(TIMES_KEY)); } catch (e) { times = null; }
  if (!times || !times.task) times = { task: {}, turn: {}, open: {}, turnStart: {} };
  if (!times.gate) times.gate = {};
  if (!times.gateOpen) times.gateOpen = {};
  // A gated command's history key: who (the gate session name) and its first two plain words.
  const gateKey = (x) => String(x.session || '').toLowerCase() + '|' +
    String(x.cmd || '').split(/\s+/).filter((w) => w && !w.startsWith('-') && !/[\\/:]/.test(w) && !/^\d/.test(w)).slice(0, 2).join(' ').toLowerCase();
  function fmt(sec) {
    sec = Math.max(0, Math.round(sec));
    if (sec < 60) return sec + ' s';
    const m = Math.round(sec / 60);
    return m < 60 ? m + ' min' : Math.floor(m / 60) + ' h ' + (m % 60) + ' min';
  }
  const taskKey = (p, label) => p + '|' + String(label || '').toLowerCase();
  function pushTime(map, key, v) { const l = map[key] || (map[key] = []); l.push(Math.round(v)); while (l.length > 12) l.shift(); }
  function typical(list) {
    if (!list || list.length < 2) return null;
    const a = list.slice().sort((x, y) => x - y);
    const q = (f) => a[Math.min(a.length - 1, Math.floor(f * (a.length - 1) + 0.5))];
    return { median: q(0.5), slow: q(0.8), n: a.length };
  }
  const isWorking = (s) => s.st.key === 'busy';
  function recordTimes(st) {
    const now = Date.now();
    let changed = false;
    const seen = new Set();
    for (const s of st.running) for (const t of s.tasks || []) {
      if (!t.started) continue;
      const id = s.project.name + '|' + t.pid;
      seen.add(id);
      const key = taskKey(s.project.name, t.label);
      if (!times.open[id]) { times.open[id] = { key, started: Date.parse(t.started) }; changed = true; }
      else if (times.open[id].key !== key) { times.open[id].key = key; changed = true; }
    }
    for (const [id, o] of Object.entries(times.open)) {
      if (seen.has(id)) continue;
      const secs = (now - o.started) / 1000;
      if (secs > 5 && secs < 6 * 3600) pushTime(times.task, o.key, secs);
      delete times.open[id];
      changed = true;
    }
    for (const s of st.running) {
      const start = times.turnStart[s.id];
      if (isWorking(s) && !start) { times.turnStart[s.id] = s.since && s.status !== 'idle' ? s.since : now; changed = true; }
      // Claude's own status-change time is exact; an earlier one replaces a guess made before it arrived.
      else if (isWorking(s) && s.since && s.status !== 'idle' && s.since < start) { times.turnStart[s.id] = s.since; changed = true; }
      else if (!isWorking(s) && start) {
        const secs = (now - start) / 1000;
        if (secs > 20 && secs < 12 * 3600) pushTime(times.turn, s.project.name, secs);
        delete times.turnStart[s.id];
        changed = true;
      }
    }
    for (const id of Object.keys(times.turnStart)) if (!st.running.some((s) => s.id === id)) { delete times.turnStart[id]; changed = true; }
    // How long each command run through the gate takes (holds excluded: their length is the agent's choice).
    if (st.gate) {
      const live = new Set();
      for (const j of st.gate.jobs) {
        if (!j.gated || j.kind === 'hold' || !j.started) continue;
        live.add(String(j.pid));
        if (!times.gateOpen[j.pid]) { times.gateOpen[j.pid] = { key: gateKey(j), started: j.started }; changed = true; }
      }
      for (const [pid, o] of Object.entries(times.gateOpen)) {
        if (live.has(pid)) continue;
        const secs = (now - o.started) / 1000;
        if (secs > 3 && secs < 6 * 3600) pushTime(times.gate, o.key, secs);
        delete times.gateOpen[pid];
        changed = true;
      }
    }
    if (changed) try { localStorage.setItem(TIMES_KEY, JSON.stringify(times)); } catch (e) { /* storage full or off */ }
  }
  // An estimate from the usual time, while there is one. Past it, a long job is just a long job: no judgment,
  // only the time it has run (the honey stays where it got to).
  function verdict(elapsed, typ) {
    if (!typ) return { kind: 'new', text: '', frac: null };
    const m = typ.median;
    if (elapsed < m) return { kind: 'on', text: `about ${fmt(m - elapsed)} left`, frac: elapsed / m };
    return { kind: 'over', text: '', frac: 1 };
  }
  // Timing and health of every open session: turn time, each task's time against its history, and
  // signs of being stuck (a task using no CPU for minutes, or a working session writing nothing at all).
  const stuckSeen = new Set();
  // Tools that should be using CPU while they run (so a long stretch without any means trouble).
  const COMPUTE = new Set(['cargo', 'rustc', 'msbuild', 'cmake', 'ninja', 'cl', 'link', 'dotnet', 'devenv', 'gradle', 'go', 'make', 'mingw32-make',
    'clang', 'clang++', 'gcc', 'g++', 'javac', 'mvn', 'npm', 'pnpm', 'yarn', 'bun', 'deno', 'tsc', 'webpack', 'vite', 'pytest', 'cargo-nextest']);
  const isCompute = (t) => COMPUTE.has(String(t.label || '').split(/\s+/)[0].toLowerCase().replace(/\.exe$/, ''));
  function annotate(st) {
    const now = Date.now();
    for (const s of st.running) {
      const since = s.since || null;
      s.waitFor = !isWorking(s) && since ? (now - since) / 1000 : null;
      const start = times.turnStart[s.id];
      s.turnSecs = isWorking(s) && start ? (now - start) / 1000 : null;
      s.turnV = s.turnSecs != null ? verdict(s.turnSecs, typical(times.turn[s.project.name])) : null;
      for (const t of s.tasks || []) {
        t.secs = t.started ? (now - Date.parse(t.started)) / 1000 : null;
        t.v = t.secs != null ? verdict(t.secs, typical(times.task[taskKey(s.project.name, t.label)])) : null;
        t.quiet = t.cpuAt ? (now - t.cpuAt) / 1000 : null;
      }
      s.health = null;
      // "No CPU for minutes" only means hung for a compute tool (a build, a compiler, a test runner). A watcher
      // (gh run watch, a sleep, a remote wait) is supposed to sit still, and running one is a deliberate wait,
      // as is a kitten line in the waiting mood.
      const tasks = s.tasks || [];
      const hung = tasks.find((t) => !t.paused && isCompute(t) && t.quiet != null && t.quiet > 300 && t.secs > 360);
      const activeTask = tasks.some((t) => t.quiet != null && t.quiet < 120);
      const waitingOnPurpose = !!(s.queued || s.pausedJob) || tasks.some((t) => !isCompute(t)) || st.kitten.some((k) => k.mood === 'waiting' && sessionKitten(st, s) === k);
      const silent = s.activity ? (now - s.activity) / 1000 : null;
      // Waiting in the build queue (or paused by it) is never stuck, however long the turn has run.
      if (s.queued || s.pausedJob) s.health = { level: 'warn', stuck: false, why: s.queued ? 'waiting its turn in the build queue' : 'its build is paused until there is room' };
      else if (hung) s.health = { level: 'bad', stuck: true, why: `${hung.label} has used no CPU for ${fmt(hung.quiet)}` };
      else if (s.status === 'busy' && silent != null && silent > 600 && !activeTask && !waitingOnPurpose) s.health = { level: 'bad', stuck: true, why: `nothing written for ${fmt(silent)}` };
      // How long something runs is never a health signal (long jobs are just long); only signs of life are.
      else if (s.status === 'busy' && silent != null && silent > 300 && !activeTask && !waitingOnPurpose) s.health = { level: 'warn', stuck: false, why: `quiet for ${fmt(silent)}` };
      else if (s.st.key === 'asks' && s.waitFor > 600) s.health = { level: 'warn', stuck: false, why: `has been asking you for ${fmt(s.waitFor)}` };
    }
    for (const s of st.running) {
      if (s.health && s.health.stuck) {
        st.needs.unshift({ tone: s.health.level, title: `${sessName(s)} looks stuck`, sub: s.health.why, proj: s.project, key: 'stuck' + s.id, pid: s.pid });
        if (!stuckSeen.has(s.id)) { stuckSeen.add(s.id); if (!document.hasFocus()) post('attention'); }
      } else stuckSeen.delete(s.id);
    }
  }
  const etaBar = (v, label) => (v && v.kind === 'over' ? `<div class="eta over"><span>${esc(label)}</span></div>` : v && v.frac != null
    ? `<div class="eta ${v.kind}"><div class="meter"><span style="width:${Math.min(100, v.frac * 100).toFixed(0)}%"></span></div><span>${esc(label)}${v.text ? ' · ' + esc(v.text) : ''}</span></div>`
    : '');

  // ---------- the hive: cells that sessions build, finish, and leave behind ----------
  const HW = 1400, HH = 230, R = 46, CX = 1270, CY = 100;
  const hexXY = (q, r) => [CX + R * 1.5 * q, CY + R * Math.sqrt(3) * (r + q / 2)];
  const hexPath = (x, y, rr) => { let d = ''; for (let i = 0; i < 6; i++) { const a = Math.PI / 3 * i; d += (i ? 'L' : 'M') + (x + rr * Math.cos(a)).toFixed(1) + ' ' + (y + rr * Math.sin(a)).toFixed(1); } return d + 'Z'; };
  const SLOTS = [];
  for (let q = -9; q <= 2; q++) for (let r = -5; r <= 5; r++) {
    const [x, y] = hexXY(q, r);
    if (x >= 820 && x <= 1340 && y >= 50 && y <= 190) SLOTS.push(q + ',' + r);
  }
  const slotXY = (k) => { const [q, r] = k.split(',').map(Number); return hexXY(q, r); };
  // Cells only go where they are seen and clear of the greeting: the right part of what the banner shows
  // (layoutHive moves this line when the widget is resized).
  let slotMinX = 820;
  const inView = (k) => slotXY(k)[0] >= slotMinX;
  const DIRS = [[1, 0], [1, -1], [0, -1], [-1, 0], [-1, 1], [0, 1]];
  const HIVE_KEY = 'am-hive-v1';
  let hive;
  try { hive = JSON.parse(localStorage.getItem(HIVE_KEY)) || null; } catch (e) { hive = null; }
  if (!hive || !hive.cells) hive = { cells: {}, owner: {}, last: {} };
  for (const k of Object.keys(hive.cells)) if (!SLOTS.includes(k)) delete hive.cells[k];
  const moves = [];                 // flights to animate after the next render: { id, from, to }
  const arrive = new Map();         // session id -> time its bee lands in a new cell
  function freeSlot(near, salt) {
    const used = new Set(Object.keys(hive.cells));
    if (near) {
      const [q, r] = near.split(',').map(Number);
      const r2 = rng(salt);
      const dirs = DIRS.slice().sort(() => r2() - 0.5);
      for (const [dq, dr] of dirs) { const k = (q + dq) + ',' + (r + dr); if (SLOTS.includes(k) && inView(k) && !used.has(k)) return k; }
    }
    const [nx, ny] = near ? slotXY(near) : [CX, CY];
    const free = SLOTS.filter((k) => inView(k) && !used.has(k)).sort((a, b) => { const [ax, ay] = slotXY(a), [bx, by] = slotXY(b); return Math.hypot(ax - nx, ay - ny) - Math.hypot(bx - nx, by - ny); });
    if (free.length) return free[0];
    // Full: the oldest finished cell makes room.
    const capped = Object.entries(hive.cells).filter(([, c]) => !c.sid).sort((a, b) => a[1].at - b[1].at);
    if (capped.length) { delete hive.cells[capped[0][0]]; return capped[0][0]; }
    return null;
  }
  function syncHive(st) {
    const now = Date.now();
    let changed = false;
    // Out of view after a resize: finished cells there are cleared; a live session's cell moves into view.
    for (const [k, c] of Object.entries(hive.cells)) {
      if (inView(k)) continue;
      delete hive.cells[k];
      if (c.sid && hive.owner[c.sid] === k) delete hive.owner[c.sid];
      changed = true;
    }
    for (const s of st.running) {
      const key = s.st.key, prev = hive.last[s.id];
      let cell = hive.owner[s.id];
      if (!cell || !hive.cells[cell] || hive.cells[cell].sid !== s.id) {
        cell = freeSlot(null, seedOf(s.id));
        if (!cell) continue;
        hive.cells[cell] = { sid: s.id, at: now };
        hive.owner[s.id] = cell;
        changed = true;
      } else if (prev && prev !== 'busy' && key === 'busy') {
        // Back to work after being idle: the finished cell stays capped, the bee moves on to build a new one.
        hive.cells[cell] = { sid: null, at: now };
        const next = freeSlot(cell, seedOf(s.id) + now);
        if (next) {
          hive.cells[next] = { sid: s.id, at: now };
          hive.owner[s.id] = next;
          moves.push({ id: s.id, from: cell, to: next });
          arrive.set(s.id, now + 1500);
        } else {
          hive.cells[cell] = { sid: s.id, at: now };
        }
        changed = true;
      }
      if (prev !== key) { hive.last[s.id] = key; changed = true; }
    }
    for (const id of Object.keys(hive.owner)) {
      if (st.running.some((s) => s.id === id)) continue;
      const cell = hive.owner[id];
      // A finished cell stays as part of the hive; one still being built is abandoned.
      if (hive.cells[cell]) { if (hive.last[id] !== 'busy') hive.cells[cell] = { sid: null, at: now }; else delete hive.cells[cell]; }
      delete hive.owner[id]; delete hive.last[id];
      changed = true;
    }
    if (changed) try { localStorage.setItem(HIVE_KEY, JSON.stringify(hive)); } catch (e) { /* private mode */ }
  }
  // How full a session's cell is: its progress on the current work, from the best signal there is. Never full
  // (at most 85%), so the cell is always still being filled; an idle session keeps its last level (at least 60%).
  const lastLevel = new Map();
  function fillLevel(st, s) {
    const CAP = 0.85, FLOOR = 0.08;
    const scale = (p) => FLOOR + Math.max(0, Math.min(1, p)) * (CAP - FLOOR);
    if (s.st.key !== 'busy') {
      const v = Math.max(lastLevel.get(s.id) || 0, 0.6 * CAP / 0.85);
      return { v: Math.min(CAP, v), why: 'this turn is done' };
    }
    const k = sessionKitten(st, s);
    const task = (s.tasks || []).find((x) => x.v && x.v.frac != null);
    let v, why;
    if (k && k.progress != null) { v = scale(k.progress); why = `kitten progress ${k.progressLabel || Math.round(k.progress * 100) + '%'}`; }
    else if (task) { v = scale(task.v.frac); why = `${task.label}: ${fmt(task.secs)} against its usual time`; }
    else if (s.turnV && s.turnV.frac != null) { v = scale(s.turnV.frac); why = `this turn against the usual turn here`; }
    else if (s.turnSecs != null) { v = scale(1 - Math.exp(-s.turnSecs / 600)); why = `${fmt(s.turnSecs)} of work so far`; }
    else { v = FLOOR; why = 'just started'; }
    lastLevel.set(s.id, v);
    return { v, why };
  }
  // The part of a flat-top hexagon (center 0,0, radius r) below the line at fill level `lvl` (0 empty, 1 full).
  function hexBelow(r, lvl) {
    const h = r * Math.sqrt(3) / 2, y = h - lvl * 2 * h;
    const xAt = (yy) => r - Math.abs(yy) / Math.sqrt(3);
    const pts = [];
    if (y <= 0) {
      pts.push([-xAt(y), y], [xAt(y), y], [r, 0], [r / 2, h], [-r / 2, h], [-r, 0]);
    } else {
      pts.push([-xAt(y), y], [xAt(y), y], [r / 2, h], [-r / 2, h]);
    }
    return 'M' + pts.map(([a, b]) => a.toFixed(1) + ' ' + b.toFixed(1)).join('L') + 'Z';
  }
  // ---------- the hive, drawn on one canvas ----------
  // Lean: one canvas and frames only as often as the motion needs (about 30 a second while bees work, fewer when
  // the hive is calm or the widget is not focused, none while hidden); the comb and the finished cells are drawn
  // once to an offscreen image and reused. Everything moves from its own traits and a mix of waves, so no two
  // cells or bees move in step and nothing visibly repeats.
  const map = { s: 1, ox: 0, oy: 0, w: 0, h: 0, dpr: 1 };
  let relayoutTimer = 0;
  const H = { cells: [], capped: [], hover: null, pressed: null, flights: [], bgKey: '', bg: null, colors: null, levels: new Map(), hoverT: new Map(), frozen: new Map() };
  const heroObserver = window.ResizeObserver ? new ResizeObserver(() => layoutHive()) : null;
  let observedHero = null;
  function cssColors() {
    const cs = getComputedStyle(document.documentElement), g = (n) => cs.getPropertyValue(n).trim();
    return { cell: g('--cell'), line: g('--cell-line'), honey: g('--honey'), ready: g('--ready'), bad: g('--bad') };
  }
  function layoutHive() {
    const hero = $('hero'), cv = $('hivecv');
    if (!hero || !cv) return;
    const w = hero.clientWidth, h = hero.clientHeight;
    map.w = w; map.h = h; map.dpr = window.devicePixelRatio || 1;
    map.s = Math.max(w / HW, h / HH);
    map.ox = w - HW * map.s;
    map.oy = (h - HH * map.s) / 2;
    const pw = Math.round(w * map.dpr), ph = Math.round(h * map.dpr);
    if (cv.width !== pw || cv.height !== ph) { cv.width = pw; cv.height = ph; H.bg = null; }
    // Cells stay in the right half of what is shown (the greeting has the left), whole cells only.
    const min = Math.max(820, HW - (w / map.s) * 0.5 + R * 0.4);
    if (Math.abs(min - slotMinX) > 1) {
      slotMinX = min;
      clearTimeout(relayoutTimer);
      relayoutTimer = setTimeout(() => render(false), 0);
    }
    wakeHive();
  }
  // The hive's model for the canvas (called while the page renders); returns the canvas element.
  function hiveLayers(st) {
    const byId = new Map(st.running.map((s) => [s.id, s]));
    const cells = [], capped = [];
    for (const [k, c] of Object.entries(hive.cells)) {
      const [x, y] = slotXY(k);
      const s = c.sid ? byId.get(c.sid) : null;
      if (!s) { capped.push({ k, x, y }); continue; }
      const fill = fillLevel(st, s);
      const nm = sessName(s);
      cells.push({
        id: s.id, k, x, y, t: traits(s.id), key: s.st.key, pid: s.pid, hint: s.project.name, sname: s.name || '', stuck: !!(s.health && s.health.stuck),
        justUntil: justUntil.get(s.id) || 0, target: fill.v, name: nm.length > 11 ? nm.slice(0, 10) + '…' : nm,
        title: `${nm} · ${s.project.name} · ${s.st.label}${s.turnSecs != null ? ' · ' + fmt(s.turnSecs) : ''}${s.health ? ' · ' + s.health.why : ''} · honey: ${fill.why} (click to bring its window to the front)`,
        subs: (s.subagents || []).slice(0, 3).map((a) => traits(a.id)), landing: arrive.get(s.id) || 0,
      });
    }
    H.cells = cells;
    const key = capped.map((c) => c.k).join('|');
    if (key !== H.bgKey) { H.bgKey = key; H.capped = capped; H.bg = null; }
    wakeHive();
    return '<canvas class="hivecv" id="hivecv" data-key="cv" aria-hidden="true"></canvas>';
  }
  function hexCtx(c, x, y, r) {
    c.beginPath();
    for (let i = 0; i < 6; i++) { const a = Math.PI / 3 * i; const px = x + r * Math.cos(a), py = y + r * Math.sin(a); if (i) c.lineTo(px, py); else c.moveTo(px, py); }
    c.closePath();
  }
  function honeyGradient(c, top, bottom) {
    const g = c.createLinearGradient(0, top, 0, bottom);
    g.addColorStop(0, '#ffd36b'); g.addColorStop(0.35, '#f4b334'); g.addColorStop(1, '#e09a12');
    return g;
  }
  // The comb and the cells sessions left behind: drawn once per size, theme or change.
  function drawBackground(cv) {
    const off = document.createElement('canvas');
    off.width = cv.width; off.height = cv.height;
    const c = off.getContext('2d'), col = H.colors;
    c.setTransform(map.dpr * map.s, 0, 0, map.dpr * map.s, map.dpr * map.ox, map.dpr * map.oy);
    c.lineWidth = 1.5;
    for (let q = -11; q <= 3; q++) for (let r = -6; r <= 6; r++) {
      const [x, y] = hexXY(q, r);
      if (y < -R || y > HH + R || x < 560) continue;
      c.globalAlpha = Math.max(0.12, 1 - Math.hypot(x - 1150, (y - CY) * 1.6) / 620);
      hexCtx(c, x, y, R - 3);
      c.fillStyle = col.cell; c.fill();
      c.strokeStyle = col.line; c.stroke();
    }
    c.globalAlpha = 0.62;
    for (const k of H.capped) {
      c.save(); c.translate(k.x, k.y);
      // A finished cell is full to the brim.
      hexCtx(c, 0, 0, R - 3); c.fillStyle = honeyGradient(c, -R, R); c.fill();
      hexCtx(c, 0, 0, R - 4); c.strokeStyle = col.honey; c.lineWidth = 2.4; c.stroke();
      c.restore();
    }
    H.bg = off;
  }
  // One bee from its traits, facing `dir` (1 right, -1 left).
  function drawBee(c, t, x, y, time, size, dir, droop, pollen) {
    const ink = '#2b1a00';
    c.save();
    c.translate(x, y);
    c.scale(dir * t.scale * size, t.scale * size);
    c.rotate((t.tilt * Math.PI / 180) * 0.5 + (droop ? 0.35 : 0));
    const rx = 9 * t.plump, ry = 6, w = t.wing;
    // Wings beat smoothly at the bee's own two rates.
    const flapA = 0.62 + 0.38 * Math.sin(time * Math.PI / (droop ? 0.6 : t.flapA));
    const flapB = 0.62 + 0.38 * Math.sin(time * Math.PI / (droop ? 0.6 : t.flapB) + 1.3);
    c.fillStyle = 'rgba(255,255,255,0.78)'; c.strokeStyle = 'rgba(60,40,0,0.35)'; c.lineWidth = 0.8;
    for (const [cx, a, fl] of [[-2, -8 + t.wingTilt, flapA], [3, 10 + t.wingTilt, flapB]]) {
      c.save(); c.translate(cx, -1); c.rotate(a * Math.PI / 180); c.scale(1, fl);
      c.beginPath(); c.ellipse(0, -5 * w, 4 * w, 6 * w, 0, 0, Math.PI * 2); c.fill(); c.stroke(); c.restore();
    }
    c.beginPath(); c.ellipse(0, 1, rx, ry, 0, 0, Math.PI * 2);
    c.fillStyle = t.body; c.fill();
    c.save(); c.clip();
    c.fillStyle = ink;
    for (const sx of (t.stripes === 3 ? [-4.6, -0.8, 3] : [-3.6, 1.4])) c.fillRect(sx - t.stripeW / 2, -ry, t.stripeW, ry * 2 + 2);
    c.restore();
    c.strokeStyle = ink; c.lineWidth = 1; c.stroke();
    c.beginPath(); c.arc(rx + t.head * 0.55, 0, t.head, 0, Math.PI * 2); c.fillStyle = ink; c.fill();
    c.beginPath(); c.arc(rx + t.head * 0.9, -1, 0.9, 0, Math.PI * 2); c.fillStyle = '#fff'; c.fill();
    c.beginPath(); c.moveTo(rx + t.head * 0.4, -t.head * 0.8); c.quadraticCurveTo(rx + t.head * 0.4 + 2, -t.head * 0.8 - 4, rx + t.head * 0.4 + 4 + t.antenna * 0.4, -t.head * 0.8 - 4.5 + t.antenna * 0.3);
    c.lineWidth = 1; c.lineCap = 'round'; c.stroke();
    c.beginPath(); c.moveTo(-rx, 1); c.lineTo(-rx - 3, 1.6); c.lineWidth = 1.4; c.stroke();
    // Pollen baskets on the hind legs.
    if (pollen > 0.02) {
      c.fillStyle = '#f39a0b'; c.lineWidth = 0.8;
      for (const px of [-2.6, 1.8]) {
        c.beginPath(); c.arc(px, ry + 1.2, 2.6 * pollen, 0, Math.PI * 2); c.fill(); c.stroke();
      }
    }
    c.restore();
  }
  // A worker bee (sub-agent) helps its session's bee: it rests in the cell, flies out to forage and comes back
  // carrying pollen, each on its own rhythm. Returns a point in cell space at home, in hive space while away.
  function workerAt(cell, st2, i, time) {
    const ph2 = (st2.seed % 991) / 157.7, [bx, by, d] = beeAt(st2, time, ph2);
    const hx = [-18, 18, 0, -9, 9][i % 5] + bx * 0.6, hy = [8, 6, 16, 20, 20][i % 5] + by * 0.5;
    const P = 13 + (st2.seed % 900) / 100, u = (time + ((st2.seed >>> 3) % 977) / 977 * P) / P;
    const n = Math.floor(u), k = u - n;
    if (cell.stuck || k < 0.32) return { x: hx, y: hy, dir: d, pollen: 0, away: false };
    if (k >= 0.84) return { x: hx, y: hy, dir: d, pollen: Math.max(0, 1 - (k - 0.84) / 0.1), away: false };
    const r = rng((st2.seed ^ Math.imul(n + 1, 0x9e3779b1)) >>> 0);
    const side = r() < 0.5 ? -1 : 1;
    const ax = cell.x + hx, ay = cell.y + hy;
    // Forage within the part of the banner that is on screen.
    const left = Math.max(30, -map.ox / map.s + 24);
    const tx = Math.max(left, Math.min(HW - 30, ax + side * (100 + r() * 90)));
    const ty = Math.max(18, Math.min(HH - 18, ay - 70 + r() * 110));
    const lift = 30 + r() * 30;
    const bez = (s, x0, y0, x1, y1, bend) => {
      const cx = (x0 + x1) / 2, cy = Math.min(y0, y1) - bend, m = 1 - s;
      return [m * m * x0 + 2 * m * s * cx + s * s * x1, m * m * y0 + 2 * m * s * cy + s * s * y1];
    };
    const ease = (s) => s * s * (3 - 2 * s);
    if (k < 0.5) {
      const [x, y] = bez(ease((k - 0.32) / 0.18), ax, ay, tx, ty, lift);
      return { x, y, dir: tx >= ax ? 1 : -1, pollen: 0, away: true };
    }
    if (k < 0.64) {
      const s = (k - 0.5) / 0.14, env = Math.sin(Math.PI * s);
      return { x: tx + bx * 0.5 * env, y: ty + by * 0.5 * env, dir: d, pollen: s, away: true };
    }
    const [x, y] = bez(ease((k - 0.64) / 0.2), tx, ty, ax, ay, lift * 0.6);
    return { x, y, dir: ax >= tx ? 1 : -1, pollen: 1, away: true };
  }
  // Where a bee is around its home point: a mix of waves at its own rates (never repeating, never in step).
  function beeAt(t, time, ph) {
    const x = 11 * Math.sin(time * t.fx1 + ph) + 5 * Math.sin(time * t.fx2 + ph * 2.1);
    const y = 7 * Math.sin(time * t.fy1 + ph * 0.7) + 3 * Math.sin(time * t.fy2 + ph * 3.3);
    const vx = 11 * t.fx1 * Math.cos(time * t.fx1 + ph) + 5 * t.fx2 * Math.cos(time * t.fx2 + ph * 2.1);
    return [x, y, vx >= 0 ? 1 : -1];
  }
  function drawCell(c, cell, now, time) {
    const t = cell.t, col = H.colors, ph = (t.seed % 997) / 158.7;
    // A stuck cell holds still: its clock stops where it got stuck.
    if (cell.stuck) { if (!H.frozen.has(cell.id)) H.frozen.set(cell.id, time); } else H.frozen.delete(cell.id);
    const tt = H.frozen.has(cell.id) ? H.frozen.get(cell.id) : time;
    // Eased state: hover lift, honey level.
    let hv = H.hoverT.get(cell.k) || 0;
    hv += ((H.hover === cell.k ? 1 : 0) - hv) * 0.22;
    H.hoverT.set(cell.k, hv);
    let lv = H.levels.get(cell.id);
    lv = lv == null ? cell.target : lv + (cell.target - lv) * 0.035;
    H.levels.set(cell.id, lv);
    c.save();
    c.translate(cell.x, cell.y - 4 * hv);
    let sc = 1 + 0.07 * hv - (H.pressed === cell.k ? 0.07 : 0);
    const since = 4200 - (cell.justUntil - now);
    if (cell.justUntil > now && since < 900) sc *= 1 + 0.1 * Math.sin(since / 900 * Math.PI);
    if (cell.key === 'asks') c.rotate(0.09 * Math.sin(time * 9) * Math.pow(Math.max(0, Math.sin(time * t.wobble * 0.7 + ph)), 12));
    c.scale(sc, sc);
    // The honey reaches the comb's own wall (drawn at R - 3 in the background), so no gap shows at the edge,
    // also where the wall under construction has gaps.
    const r = R - 4, rIn = R - 3, h = rIn * Math.sqrt(3) / 2;
    // Liquid honey inside the hexagon: its level is the session's progress; it sloshes and its surface waves.
    c.save();
    hexCtx(c, 0, 0, rIn); c.clip();
    const calm = cell.key !== 'busy';
    const ang = (calm ? 0.025 : 0.075) * (0.7 * Math.sin(tt * t.slosh1 + ph) + 0.3 * Math.sin(tt * t.slosh2 + ph * 1.7));
    const surf = h - lv * 2 * h, amp = calm ? 0.7 : 1.5, dir = t.waveBack ? -1 : 1;
    c.beginPath(); c.moveTo(-r - 8, R + 8);
    for (let x = -r - 8; x <= r + 8; x += 3) {
      c.lineTo(x, surf + Math.tan(ang) * x + amp * Math.sin(x * 0.21 + tt * t.waveSpeed * dir + ph) + amp * 0.4 * Math.sin(x * 0.53 - tt * 1.3 + ph * 2));
    }
    c.lineTo(r + 8, R + 8); c.closePath();
    c.fillStyle = honeyGradient(c, surf - 4, h);
    c.globalAlpha = cell.stuck ? 0.55 : 0.92; c.fill(); c.globalAlpha = 1;
    c.restore();
    // Walls: under construction while working (they grow and settle, never quite close), closed when idle.
    c.lineJoin = 'round';
    if (cell.key === 'busy') {
      hexCtx(c, 0, 0, r); c.setLineDash([3, 4]); c.strokeStyle = col.line; c.lineWidth = 1.4; c.stroke();
      const per = 6 * r;
      const prog = 0.55 + 0.4 * (0.5 + 0.5 * (0.6 * Math.sin(tt * t.buildA + ph) + 0.4 * Math.sin(tt * t.buildB + ph * 2.3)));
      hexCtx(c, 0, 0, r); c.setLineDash([per * prog, per]); c.lineDashOffset = -(ph % 1) * per;
      c.strokeStyle = cell.stuck ? col.bad : col.honey; c.lineWidth = 3; c.stroke(); c.setLineDash([]); c.lineDashOffset = 0;
    } else {
      hexCtx(c, 0, 0, r); c.strokeStyle = cell.key === 'ready' ? col.ready : col.honey; c.lineWidth = 2.4; c.stroke();
      c.textAlign = 'center';
      c.font = cell.key === 'asks' ? '800 24px Figtree, Segoe UI, sans-serif' : '800 18px Figtree, Segoe UI, sans-serif';
      c.fillStyle = cell.key === 'asks' ? col.bad : col.ready;
      c.fillText(cell.key === 'asks' ? '?' : '✓', 0, cell.key === 'asks' ? 2 : -2);
    }
    c.font = '700 10.5px Figtree, Segoe UI, sans-serif'; c.textAlign = 'center';
    c.lineWidth = 3; c.strokeStyle = 'rgba(255,236,176,0.85)'; c.strokeText(cell.name, 0, R * 0.52);
    c.fillStyle = '#2b1a00'; c.fillText(cell.name, 0, R * 0.52);
    // The session's bee (after its flight has landed) and a small worker bee per sub-agent.
    if (cell.key === 'busy' && cell.landing <= now) {
      const [bx, by, d] = beeAt(t, cell.stuck ? tt * 0.25 : time, ph);
      drawBee(c, t, bx, by - 10 + (cell.stuck ? 3 : 0), time, 1, d, cell.stuck);
    }
    cell.subs.forEach((st2, i) => {
      const w = workerAt(cell, st2, i, time);
      if (w.away) H.away.push([st2, w]);
      else drawBee(c, st2, w.x, w.y, time, 0.62, w.dir, false, w.pollen);
    });
    c.restore();
  }
  function drawFlights(c, now, time) {
    H.flights = H.flights.filter((f) => now < f.t0 + f.dur);
    for (const f of H.flights) {
      let k = (now - f.t0) / f.dur;
      k = k < 0.5 ? 4 * k * k * k : 1 - Math.pow(-2 * k + 2, 3) / 2;
      const p = f.pts, n = p.length - 1, seg = Math.min(n - 1, Math.floor(k * n)), u = k * n - seg;
      // Smooth through the points (Catmull-Rom).
      const P = (i) => p[Math.max(0, Math.min(n, i))];
      const [p0, p1, p2, p3] = [P(seg - 1), P(seg), P(seg + 1), P(seg + 2)];
      const cr = (a, b, cc, dd) => 0.5 * (2 * b + (-a + cc) * u + (2 * a - 5 * b + 4 * cc - dd) * u * u + (-a + 3 * b - 3 * cc + dd) * u * u * u);
      const x = cr(p0[0], p1[0], p2[0], p3[0]), y = cr(p0[1], p1[1], p2[1], p3[1]);
      const dir = p2[0] >= p1[0] ? 1 : -1;
      drawBee(c, f.t, x, y + 2 * Math.sin(time * 7 + f.t0), time, 1, dir, false);
    }
  }
  let frameTimer = 0, rafId = 0;
  function wakeHive() { if (!frameTimer && !rafId) rafId = requestAnimationFrame(frame); }
  function frame(nowMs) {
    rafId = 0;
    const cv = $('hivecv');
    if (!cv || document.hidden || !map.w) return;
    const now = Date.now(), time = nowMs / 1000;
    if (!H.colors) H.colors = cssColors();
    if (!H.bg) drawBackground(cv);
    const c = cv.getContext('2d');
    c.setTransform(1, 0, 0, 1, 0, 0);
    c.clearRect(0, 0, cv.width, cv.height);
    c.drawImage(H.bg, 0, 0);
    c.setTransform(map.dpr * map.s, 0, 0, map.dpr * map.s, map.dpr * map.ox, map.dpr * map.oy);
    H.away = [];
    for (const cell of H.cells) drawCell(c, cell, now, time);
    // Foraging workers fly over every cell, so they are drawn after all of them.
    for (const [st2, w] of H.away) drawBee(c, st2, w.x, w.y, time, 0.62, w.dir, false, w.pollen);
    drawFlights(c, now, time);
    // Frame budget: about 30 a second while bees work or fly, 20 when the widget is not focused, 10 when calm.
    const lively = H.cells.some((x) => x.key === 'busy' || x.subs.length) || H.flights.length || H.hover;
    const fps = !lively ? 10 : document.hasFocus() ? 30 : 20;
    frameTimer = setTimeout(() => { frameTimer = 0; rafId = requestAnimationFrame(frame); }, 1000 / fps);
  }
  document.addEventListener('visibilitychange', () => { if (!document.hidden) wakeHive(); });
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', () => { H.colors = null; H.bg = null; });
  // Pointer: hover lifts a cell, click brings its session's window to the front.
  function cellUnder(e) {
    const cv = $('hivecv'), rc = cv.getBoundingClientRect();
    const x = (e.clientX - rc.left - map.ox) / map.s, y = (e.clientY - rc.top - map.oy) / map.s;
    let best = null, bd = R * 0.9;
    for (const cell of H.cells) { const d = Math.hypot(x - cell.x, y - cell.y); if (d < bd) { bd = d; best = cell; } }
    return best;
  }
  view.addEventListener('pointermove', (e) => {
    if (e.target.id !== 'hivecv') return;
    const cell = cellUnder(e);
    const k = cell ? cell.k : null;
    if (k !== H.hover) {
      H.hover = k;
      e.target.style.cursor = cell && cell.pid ? 'pointer' : '';
      e.target.title = cell ? cell.title : '';
      wakeHive();
    }
  });
  view.addEventListener('pointerleave', () => { H.hover = null; }, true);
  view.addEventListener('click', (e) => {
    if (e.target.id !== 'hivecv') return;
    const cell = cellUnder(e);
    if (!cell || !cell.pid || !app) return;
    H.pressed = cell.k;
    setTimeout(() => { H.pressed = null; }, 160);
    post('focus:' + cell.pid + '|' + (cell.hint || '') + '|' + (cell.sname || ''));
  });
  function hero(st) {
    const hr = new Date().getHours();
    const greet = hr < 5 ? 'The hive never sleeps' : hr < 12 ? 'Good morning' : hr < 17 ? 'Good afternoon' : hr < 22 ? 'Good evening' : 'Good night';
    const parts = [`<b>${st.running.length}</b> session${st.running.length === 1 ? '' : 's'} open`];
    if (st.busy.length) parts.push(`<b>${st.busy.length}</b> working`);
    if (st.asks.length) parts.push(`<b>${st.asks.length}</b> asking you`);
    if (st.ready.length) parts.push(`<b>${st.ready.length}</b> ready for you`);
    const subCount = st.running.reduce((n, s) => n + (s.subagents || []).length, 0);
    if (subCount) parts.push(`<b>${subCount}</b> sub-agent${subCount === 1 ? '' : 's'}`);
    if (st.agents.length) parts.push(`<b>${st.agents.length}</b> other agent${st.agents.length === 1 ? '' : 's'}`);
    const done = Object.values(hive.cells).filter((c) => !c.sid).length;
    if (done) parts.push(`<b>${done}</b> cell${done === 1 ? '' : 's'} finished`);
    return `<section class="hero" data-key="hero" id="hero">${hiveLayers(st)}
      <div class="hello"><h1>${greet}</h1><p>${esc(new Date().toLocaleDateString('en-US', { weekday: 'long', month: 'long', day: 'numeric' }))}</p><p>${parts.join(' · ')}</p></div></section>`;
  }
  // Flights: a bee moving to its new cell, and now and then a forager crossing the hive.
  function runMoves() {
    while (moves.length) {
      const m = moves.shift();
      const [x0, y0] = slotXY(m.from), [x1, y1] = slotXY(m.to);
      const r = rng(seedOf(m.id) + Date.now());
      const mid = [(x0 + x1) / 2 + (r() - 0.5) * 60, Math.min(y0, y1) - 30 - r() * 40];
      H.flights.push({ pts: [[x0, y0 - 10], mid, [x1, y1 - 10]], t0: Date.now(), dur: 1300 + r() * 500, t: traits(m.id) });
    }
    wakeHive();
  }
  let forager = 0;
  function scheduleForager() {
    clearTimeout(forager);
    forager = setTimeout(() => {
      if (!document.hidden && $('hivecv')) {
        const r = rng(Date.now() >>> 0);
        const fromLeft = r() < 0.5, n = 4 + Math.floor(r() * 3), pts = [];
        for (let i = 0; i <= n; i++) {
          const f = i / n;
          pts.push([fromLeft ? -40 + f * (HW + 80) : HW + 40 - f * (HW + 80), 40 + r() * 150]);
        }
        H.flights.push({ pts, t0: Date.now(), dur: 9000 + r() * 9000, t: traits('forager' + Math.floor(r() * 1e6)) });
        wakeHive();
      }
      scheduleForager();
    }, 5000 + Math.random() * 14000);
  }

  // ---------- pieces ----------
  function sessionRow(st, s, opts) {
    opts = opts || {};
    const k = s.st.key === 'busy' ? sessionKitten(st, s) : null;
    const sub = [s.aiTitle && s.aiTitle !== sessName(s) ? s.aiTitle : '', s.where ? 'in ' + s.where : ''].filter(Boolean).join(' · ');
    const tasks = (s.tasks || []).map((t) => (t.v && t.v.frac != null
      ? etaBar(t.v, `${t.label} · ${fmt(t.secs)}`)
      : `<div class="said kit"><span class="who">running</span>${esc(t.label)}${t.secs != null ? ' · ' + esc(fmt(t.secs)) : ''}${t.quiet > 120 ? ' · no CPU for ' + esc(fmt(t.quiet)) : ''}</div>`)).join('');
    const label = s.st.label + (s.turnSecs != null ? ' · ' + fmt(s.turnSecs) : s.waitFor != null && s.waitFor > 45 ? ' · ' + fmt(s.waitFor) : '');
    const health = s.health ? `<div class="said stuckline ${s.health.level}"><span class="who">${s.health.stuck ? 'stuck?' : 'note'}</span>${esc(s.health.why)}</div>` : '';
    const can = app && s.running && s.pid;
    const subs = s.running ? s.subagents || [] : [];
    const subLines = subs.map((a) => `<div class="said sub"><span class="who">${a.background ? 'background agent' : 'sub-agent'}</span>${esc(a.description || a.type || 'agent')} · ${esc(ago(a.activity))}</div>`).join('');
    return `<div class="row ${s.st.tone}${isJust(s.id) ? ' just' : ''}${can ? ' focusable' : ''}${s.health && s.health.stuck ? ' stuck' : ''}" data-key="s-${esc(s.id)}"${can ? ` data-pid="${s.pid}" data-hint="${esc(s.project.name)}" data-sname="${esc(s.name || '')}" title="Click to bring this window to the front"` : ''}>
      ${av(s.project.name, 'sm')}
      <div class="row-main">
        <div class="row-top"><b>${esc(sessName(s))}</b>${s.st.key === 'busy' ? miniBee(s.id) : ''}${s.running ? tag(s.st.tone, label) : tag('plain', 'closed')}${subs.length ? tag('busy', subs.length + ' sub-agent' + (subs.length > 1 ? 's' : '')) : ''}${s.health && s.health.stuck ? tag(s.health.level === 'bad' ? 'bad' : 'warn', 'stuck?') : ''}<span class="when">${esc(ago(s.lastWrite))}</span></div>
        <div class="sub"><a href="${projHref(s.project)}">${esc(s.project.name)}</a>${sub ? ' · ' + esc(sub) : ''}</div>
        ${health}
        ${s.turnV && s.turnV.frac != null ? etaBar(s.turnV, 'this turn ' + fmt(s.turnSecs)) : ''}
        ${tasks}
        ${subLines}
        ${k ? `<div class="said kit"><span class="who">kitten</span>${esc(k.text)}</div>${kbar(k)}` : ''}
        ${s.lastPrompt ? `<div class="said you"><span class="who">you</span>${esc(s.lastPrompt)}</div>` : ''}
        ${s.lastSaid && !opts.short ? `<div class="said"><span class="who">claude</span>${esc(s.lastSaid)}</div>` : ''}
      </div></div>`;
  }
  const card = (key, tone, title, count, link, body, extra) =>
    `<section class="card" data-key="${key}" data-tone="${tone}"${extra || ''}><div class="card-h"><span class="dot"></span><h2>${esc(title)}</h2>${count != null ? `<span class="n">${count}</span>` : ''}${link || ''}</div>${body}</section>`;
  const rowsOr = (rows, empty) => (rows.length ? `<div class="rows">${rows.join('')}</div>` : `<div class="empty">${esc(empty)}</div>`);

  // ---------- views ----------
  const views = {
    home(st) {
      const working = st.busy.map((s) => sessionRow(st, s, { short: true }));
      const used = new Set(st.busy.map((s) => sessionKitten(st, s)).filter(Boolean));
      for (const k of st.kitten) if (!used.has(k) && k.mood === 'work') working.push(`<div class="row busy" data-key="k-${esc(k.name + k.text)}">${k.proj ? av(k.proj.name, 'sm') : '<span class="av sm neutral" aria-hidden="true">?</span>'}<div class="row-main"><div class="row-top"><b>${esc(k.text)}</b><span class="when">${esc(ago(k.time))}</span></div><div class="sub">Kitten · ${esc(k.name || 'shared line, any session')}</div>${kbar(k)}</div></div>`);
      const needs = st.needs.map((n) => `<${n.url ? `a href="${esc(n.url)}" target="_blank" rel="noopener"` : 'div'} class="row ${n.tone}${n.pid && app ? ' focusable' : ''}" data-key="n-${esc(n.key)}"${n.pid && app ? ` data-pid="${n.pid}" data-hint="${esc(n.proj ? n.proj.name : '')}" title="Click to bring this window to the front"` : ''}>${n.proj ? av(n.proj.name, 'sm') : ''}<div class="row-main"><div class="row-top"><b>${esc(n.title)}</b>${n.when ? `<span class="when">${esc(ago(n.when))}</span>` : ''}</div><div class="sub">${esc([n.proj && n.proj.name, n.sub].filter(Boolean).join(' · '))}</div></div></${n.url ? 'a' : 'div'}>`);
      const maxAge = Math.max(1, ...st.projects.map((p) => (isFinite(ms(p.lastActive)) ? ms(p.lastActive) : 1)));
      const plist = st.projects.map((p) => {
        const fresh = Math.max(4, 100 - (ms(p.lastActive) / maxAge) * 100);
        const t = p._asks ? tag('bad', 'asking') : p._busy ? tag('busy', 'working') : p._open ? tag('ready', p._open + ' open') : `<span class="when">${esc(ago(p.lastActive))}</span>`;
        return `<a class="prow" href="${projHref(p)}" data-key="p-${esc(p.name)}">${av(p.name, 'sm')}<div class="row-main"><div class="row-top"><b>${esc(p.name)}</b>${t}</div><div class="meter"><span style="width:${fresh.toFixed(0)}%"></span></div></div></a>`;
      });
      return hero(st) + `<div class="grid-home"><div class="col">
          ${st.asks.length ? card('asks', 'bad', 'Needs your answer', st.asks.length, '', rowsOr(st.asks.map((s) => sessionRow(st, s)), '')) : ''}
          ${card('working', 'busy', 'Working', working.length, '', rowsOr(working, 'Nothing is running. The hive is resting.'))}
          ${card('ready', 'ready', 'Ready for you', st.ready.length, '<a class="link" href="#sessions">All sessions</a>', '<div class="hint">Finished their turn and waiting on your reply</div>' + rowsOr(st.ready.map((s) => sessionRow(st, s)), 'No session is waiting on you.'))}
        </div><div class="col">
          ${gateCard(st)}
          ${othersCard(st)}
          ${card('open', '', 'The hive', null, '', `<div class="big">${st.running.length}<small>sessions open</small></div><div class="tags">${tag('busy', st.busy.length + ' working')}${st.asks.length ? tag('bad', st.asks.length + ' asking') : ''}${tag('ready', st.ready.length + ' ready')}${tag('plain', st.projects.length + ' projects this month')}</div>`)}
          ${card('look', st.needs.some((n) => n.tone === 'bad') ? 'bad' : st.needs.length ? 'warn' : '', 'Look at', st.needs.length || null, '', rowsOr(needs, 'All clear: no failing CI, no stuck kitten, nothing behind.'))}
          ${card('projects', '', 'Projects', st.projects.length, '<a class="link" href="#projects">Open</a>', '<div class="hint">Bar = how recently it was active</div><div class="plist">' + plist.join('') + '</div>')}
        </div></div>`;
    },
    sessions(st) {
      const order = st.asks.concat(st.busy, st.ready);
      const cards = order.map((s) => card('s-' + s.id, s.st.tone, s.project.name, null, '', sessionRow(st, s)));
      const closed = st.sessions.filter((s) => !s.running && ms(s.lastWrite) < 3 * 86400e3).sort((a, b) => ms(a.lastWrite) - ms(b.lastWrite)).slice(0, 14);
      return `<div class="head"><h1>Sessions</h1><p>Every open Claude Code window, what it is doing, and what you last said to it.${app ? ' Click one to bring its window to the front.' : ''}</p></div>
        <div class="cards">${cards.join('') || card('none', '', 'No open sessions', null, '', '<div class="empty">No Claude Code window is open.</div>')}</div>
        <div style="margin-top:22px">${card('closed', '', 'Closed in the last 3 days', closed.length, '', rowsOr(closed.map((s) => sessionRow(st, s, { short: true })), 'None.'))}</div>`;
    },
    projects(st, arg) {
      const sel = st.projects.find((p) => p.name === arg) || st.projects[0];
      const picks = st.projects.map((p) => {
        const state = p._asks ? 'asking you' : p._busy ? 'working' : p._open ? p._open + ' open' : 'active ' + ago(p.lastActive);
        const tone = p._asks ? 'bad' : p._busy ? 'busy' : p._open ? 'ready' : '';
        const tags = [p.git ? tag('plain', p.git.branch) : '', p._dirty ? tag('warn', p._dirty + ' uncommitted') : '', p.prs.length ? tag('', p.prs.length + ' PR' + (p.prs.length > 1 ? 's' : '')) : '',
          p.runs.some((r) => r.result === 'fail') ? tag('bad', 'CI failing') : ''].join('');
        return `<button class="card pick" data-key="pk-${esc(p.name)}" data-tone="${tone}" data-name="${esc(p.name)}" aria-pressed="${p === sel}">${av(p.name)}<div class="row-main"><div class="state">${esc(state)}</div><h3>${esc(p.name)}</h3><div class="tags">${tags}</div></div></button>`;
      });
      return `<div class="head"><h1>Projects</h1><p>Pick a project to see its sessions, git, PRs and where you left off.</p></div>
        <div class="split"><div class="picks">${picks.join('')}</div><aside class="card detail" id="detail" data-key="detail-${esc(sel ? sel.name : '')}">${sel ? detail(st, sel) : ''}</aside></div>`;
    },
    prs(st) {
      const cards = st.projects.filter((p) => p.prs.length || p.runs.length).map((p) =>
        card('pr-' + p.name, p.runs.some((r) => r.result === 'fail') || p.prs.some((x) => x.checks === 'fail') ? 'bad' : '', p.name, p.prs.length || null, '', `<div class="rows">${prRows(p)}</div>`));
      return `<div class="head"><h1>Pull requests and CI</h1><p>Open PRs on your repos, your PRs upstream, and each repo's latest CI run. Checked every 10 minutes.</p></div>
        <div class="cards">${cards.join('') || card('none', '', 'Nothing open', null, '', '<div class="empty">No open PRs or CI runs found.</div>')}</div>`;
    },
    queue(st) {
      const p = gateParts(st);
      const head = `<div class="head"><h1>Orchestration queue</h1><p>Heavy work from every agent (builds, tests, long runs) takes turns here, so nothing piles up on the machine.${app ? ' Click a row to bring that agent\'s window to the front.' : ''}</p></div>`;
      if (!p) return head + card('none', '', 'The gate is starting', null, '', '<div class="empty">No queue data yet.</div>');
      const howto = `<div class="rows mono-help">
        <div class="sub"><b>agentmon-run -- &lt;command&gt;</b> · wait for a turn, then run it</div>
        <div class="sub"><b>agentmon-run hold --for 45m</b> · keep one slot for a series, then <b>agentmon-run release</b></div>
        <div class="sub"><b>agentmon-run batch -- "a" "b"</b> · several commands in one turn</div>
        <div class="sub"><b>--exclusive</b> keeps the machine clear · <b>--lock auto</b> keeps jobs on one repo apart</div></div>`;
      return head + `<div class="grid-home"><div class="col">
          ${card('q-now', p.g.jobs.length ? 'busy' : '', 'Running now', p.g.jobs.length || null, '', p.gauges + p.now)}
          ${card('q-how', '', 'How agents join', null, '', howto)}
        </div><div class="col">
          ${card('q-next', p.g.waiting.length ? 'warn' : '', 'Up next', p.g.waiting.length || null, '', '<div class="hint">In turn order. Start times come from how long the same command took before.</div>' + p.next)}
        </div></div>`;
    },
  };

  // Every other agent on the machine (Claude runs without a window, other agent tools) and background jobs.
  function othersCard(st) {
    if (!st.agents.length && !st.jobs.length) return '';
    const now = Date.now();
    const agentRows = st.agents.map((a) => {
      const active = a.cpuAt && now - a.cpuAt < 120000;
      return `<div class="row ${active ? 'busy' : 'ready'}${app ? ' focusable' : ''}" data-key="ag-${a.pid}"${app ? ` data-pid="${a.pid}" data-hint="" title="Click to bring its window to the front"` : ''}>
        <span class="av sm neutral" aria-hidden="true">${esc(a.kind.slice(0, 2))}</span><div class="row-main">
        <div class="row-top"><b>${esc(a.kind)}</b>${tag(active ? 'busy' : 'ready', active ? 'working' : 'quiet')}<span class="when">${a.started ? esc(fmt((now - a.started) / 1000)) : ''}</span></div>
        <div class="sub">process ${a.pid}</div></div></div>`;
    });
    const jobRows = st.jobs.map((j) => `<div class="row ${j.needs ? 'warn' : 'busy'}" data-key="jb-${esc(j.id)}"><div class="row-main">
        <div class="row-top"><b>${esc(j.detail || 'Background job')}</b>${tag(j.needs ? 'warn' : 'plain', j.state || 'running')}<span class="when">${esc(ago(j.updated))}</span></div>
        ${j.needs ? `<div class="said kit"><span class="who">needs you</span>${esc(j.needs)}</div>` : ''}</div></div>`);
    return card('others', jobRows.some((r) => r.includes('needs you')) ? 'warn' : '', 'Other agents and jobs', st.agents.length + st.jobs.length, '',
      `<div class="rows">${agentRows.join('')}${jobRows.join('')}</div>`);
  }

  // The usual length of a gated command (one earlier run is enough to go on), in seconds.
  function gateTypical(x) {
    const l = times.gate[gateKey(x)];
    if (!l || !l.length) return null;
    const a = l.slice().sort((p, q) => p - q);
    return a[Math.floor(a.length / 2)];
  }
  // When each waiting entry should start: the running work's time left (a hold: until it runs out), then
  // the usual length of everything ahead of it, spread over the free slots. null when part of it is unknown.
  function queueEtas(g) {
    const now = Date.now();
    const free = [];
    for (const j of g.jobs) {
      if (j.paused || !j.gated) continue;
      if (j.kind === 'hold') free.push(j.expires ? Math.max(0, (j.expires - now) / 1000) : null);
      else { const t = gateTypical(j); free.push(t == null ? null : Math.max(30, t - (now - j.started) / 1000)); }
    }
    while (free.length < Math.max(1, g.slots)) free.push(0);
    return g.waiting.map((w) => {
      if (free.some((f) => f == null)) return null;
      free.sort((a, b) => a - b);
      const at = free[0];
      const t = w.kind === 'hold' ? null : gateTypical(w);
      free[0] = t == null ? null : at + t;
      return at;
    });
  }
  // The build gate: machine gauges, what holds a slot, what waits for one (with when it should start).
  function gateParts(st) {
    const g = st.gate;
    if (!g) return null;
    const byId = new Map(st.sessions.map((s) => [s.id, s]));
    const who = (x) => { const s = x.sid && byId.get(x.sid); return s ? sessName(s) : x.session || x.exe || 'unknown'; };
    const proj = (x) => { const s = x.sid && byId.get(x.sid); return s ? s.project.name : ''; };
    const used = g.jobs.filter((j) => !j.paused).length;
    const cpuTone = g.cpu >= g.maxCpu ? 'warn' : '';
    const memTone = g.freeGb < g.minFreeGb ? 'warn' : '';
    const gauge = (label, frac, text, tone) => `<div class="gauge ${tone}"><span class="label">${label}</span><div class="meter"><span style="width:${Math.min(100, Math.max(2, frac * 100)).toFixed(0)}%"></span></div><span class="gv">${esc(text)}</span></div>`;
    const jobRow = (j) => {
      const secs = j.started ? (Date.now() - j.started) / 1000 : null;
      const hold = j.kind === 'hold';
      const what = hold ? `slot reserved for a series of commands${j.expires ? ' · ' + fmt((j.expires - Date.now()) / 1000) + ' left' : ''}` : j.gated ? j.cmd : j.exe;
      const tags = [hold ? tag('busy', 'reserved') : j.kind === 'batch' ? tag('good', 'batch') : j.gated ? tag('good', 'through the gate') : j.game ? tag('plain', 'game') : tag('warn', 'outside the gate'),
        j.exclusive ? tag('bad', 'exclusive') : '', (j.locks || []).length ? tag('plain', 'lock: ' + j.locks.map((l) => l.split(/[\\/]/).pop()).join(', ')) : '',
        j.paused ? tag('bad', 'paused by agentMon · ' + fmt((Date.now() - j.paused) / 1000)) : ''].join('');
      const pid = app && j.sid && byId.get(j.sid) ? byId.get(j.sid).pid : 0;
      return `<div class="row ${j.paused ? 'warn' : 'busy'}${pid ? ' focusable' : ''}" data-key="j-${j.pid}"${pid ? ` data-pid="${pid}" data-hint="${esc(proj(j))}"` : ''}>${proj(j) ? av(proj(j), 'sm') : '<span class="av sm neutral" aria-hidden="true">⚙</span>'}<div class="row-main">
        <div class="row-top"><b>${esc(who(j))}</b><span class="when">${secs != null ? esc(fmt(secs)) : ''}</span></div>
        <div class="sub mono-line">${esc(what)}</div><div class="tags">${tags}</div></div></div>`;
    };
    const etas = queueEtas(g);
    const waitRow = (w, i) => {
      const pid = app && w.sid && byId.get(w.sid) ? byId.get(w.sid).pid : 0;
      const eta = etas[i];
      const when = eta == null ? (i ? `after ${i} ahead` : 'next') : eta < 45 ? 'starts any moment' : `starts in about ${fmt(eta)}`;
      const what = w.kind === 'hold' ? 'a slot for a series of commands' : w.cmd;
      const tags = [w.kind === 'hold' ? tag('busy', 'hold') : w.kind === 'batch' ? tag('good', 'batch') : '', w.exclusive ? tag('bad', 'exclusive') : '',
        (w.locks || []).length ? tag('plain', 'lock: ' + w.locks.map((l) => l.split(/[\\/]/).pop()).join(', ')) : '', tag('plain', when)].join('');
      return `<div class="row warn${pid ? ' focusable' : ''}" data-key="w-${w.pid}"${pid ? ` data-pid="${pid}" data-hint="${esc(proj(w))}"` : ''}><span class="qpos">${i + 1}</span><div class="row-main">
        <div class="row-top"><b>${esc(who(w))}</b><span class="when">waiting ${esc(fmt((Date.now() - w.created) / 1000))}</span></div>
        <div class="sub mono-line">${esc(what)}</div><div class="tags">${tags}</div></div></div>`;
    };
    const gauges = `<div class="gauges">
        ${gauge('CPU', g.cpu / 100, Math.round(g.cpu) + '%', cpuTone)}
        ${gauge('Memory', 1 - g.freeGb / Math.max(1, g.totalGb), g.freeGb.toFixed(1) + ' GB free', memTone)}
        ${gauge('Slots', used / Math.max(1, g.slots), used + ' of ' + g.slots + ' in use', used >= g.slots ? 'warn' : '')}</div>`;
    const now = g.jobs.length ? `<div class="rows">${g.jobs.map(jobRow).join('')}</div>` : '<div class="empty">No heavy work running.</div>';
    const next = g.waiting.length ? `<div class="rows">${g.waiting.map(waitRow).join('')}</div>` : '<div class="empty">Nothing waiting.</div>';
    const tone = g.jobs.some((j) => j.paused) || g.waiting.length ? 'warn' : g.jobs.length ? 'busy' : '';
    return { g, gauges, now, next, tone };
  }
  function gateCard(st) {
    const p = gateParts(st);
    if (!p) return '';
    const body = p.gauges + p.now + (p.g.waiting.length ? `<div class="label" style="margin:12px 0 6px">Up next</div>${p.next}` : '');
    return card('gate', p.tone, 'Orchestration queue', p.g.jobs.length + p.g.waiting.length || null, '<a class="link" href="#queue">Open</a>', body);
  }

  function prRows(p) {
    const prs = p.prs.map((x) => {
      const ck = x.checks === 'fail' ? tag('bad', 'checks failing') : x.checks === 'running' ? tag('warn', 'checks running') : x.checks === 'pass' ? tag('good', 'checks pass') : '';
      return `<a class="row ${x.checks === 'fail' ? 'bad' : ''}" data-key="x-${esc(x.url)}" href="${esc(x.url)}" target="_blank" rel="noopener"><div class="row-main"><div class="row-top"><b>#${x.number} ${esc(x.title)}</b></div><div class="tags">${x.draft ? tag('plain', 'draft') : ''}${ck}${tag('plain', x.branch)}<span class="small muted">${esc(x.repo)}</span></div></div></a>`;
    });
    const runs = p.runs.map((r) => `<a class="row ${r.result === 'fail' ? 'bad' : ''}" data-key="r-${esc(r.url)}" href="${esc(r.url)}" target="_blank" rel="noopener"><div class="row-main"><div class="row-top"><b>${esc(r.workflow)}</b>${tag(r.result === 'pass' ? 'good' : r.result === 'fail' ? 'bad' : 'warn', r.state.replace('_', ' '))}<span class="when">${esc(ago(r.created))}</span></div><div class="sub">Latest CI run · ${esc(r.repo)} on ${esc(r.branch)}</div></div></a>`);
    return prs.join('') + runs.join('');
  }

  function detail(st, p) {
    const g = p.git;
    const sess = st.sessions.filter((s) => s.project === p).sort((a, b) => (b.running - a.running) || ms(a.lastWrite) - ms(b.lastWrite));
    const kl = kittenFor(st, p);
    const failing = p.runs.filter((r) => r.result === 'fail').length + p.prs.filter((x) => x.checks === 'fail').length;
    let h = `<div class="dhead">${av(p.name, 'lg')}<div class="row-main"><div class="label">Last active ${esc(ago(p.lastActive))}</div><h2>${esc(p.name)}</h2><div class="path">${esc(p.root)}</div></div></div>
      <div class="stats">
        <div class="stat ${p._busy ? 'busy' : ''}"><b>${p._open}</b><span>open sessions</span></div>
        <div class="stat ${p._dirty ? 'warn' : ''}"><b>${p._dirty}</b><span>uncommitted files</span></div>
        <div class="stat"><b>${p.prs.length}</b><span>open PRs</span></div>
        <div class="stat ${failing ? 'bad' : ''}"><b>${failing}</b><span>failing checks</span></div></div>`;
    if (kl.length) h += `<div class="sec"><span class="label">Kitten</span><div class="rows">${kl.map((k) => `<div class="row ${k.mood === 'problem' ? 'bad' : k.mood === 'waiting' ? 'warn' : 'busy'}"><div class="row-main"><div class="row-top"><b>${esc(k.text)}</b><span class="when">${esc(ago(k.time))}</span></div><div class="sub">${esc(k.name)}</div>${kbar(k)}</div></div>`).join('')}</div></div>`;
    if (p.leftOff.lines.length || p.leftOff.todoCount) h += `<div class="sec"><span class="label">Where you left off</span>${p.leftOff.lines.map((l) => `<div class="note">${esc(l)}</div>`).join('')}${p.leftOff.todoCount ? `<div class="small muted" style="margin-top:6px">${p.leftOff.todoCount} todo files in .claude\\todo</div>` : ''}</div>`;
    h += `<div class="sec"><span class="label">Claude sessions</span>${rowsOr(sess.map((s) => sessionRow(st, s)), 'None.')}</div>`;
    if (g) {
      const ab = [g.ahead ? g.ahead + ' ahead' : '', g.behind ? g.behind + ' behind' : ''].filter(Boolean).join(', ');
      h += `<div class="sec"><span class="label">Git</span><div class="tags">${tag('plain', g.branch)}${g.changed ? tag('warn', g.changed + ' changed') : tag('good', 'clean')}${ab ? `<span class="small muted">${esc(ab)} ${esc(g.upstream)}</span>` : ''}</div>`;
      if (g.worktrees.length) {
        const dirty = g.worktrees.filter((w) => w.changed);
        h += `<div class="small muted" style="margin-top:8px">${g.worktrees.length} worktrees, ${dirty.length} with uncommitted work</div>` +
          dirty.map((w) => `<div class="wt"><code>${esc(w.leaf)}</code><span class="muted">${esc(w.branch)}</span><span class="n">${w.changed} changed</span></div>`).join('');
      }
      h += '</div>';
    }
    if (p.prs.length || p.runs.length) h += `<div class="sec"><span class="label">PRs and CI</span><div class="rows">${prRows(p)}</div></div>`;
    return h;
  }

  // ---------- in-place patching ----------
  // Updates the live DOM to match new markup: same-key / same-position nodes are patched, so running
  // animations (bees, building cells) keep going and nothing flashes. New keyed HTML rows slide in.
  function morph(a, b) {
    if (a.nodeType !== b.nodeType || a.nodeName !== b.nodeName) { a.replaceWith(b); return; }
    if (a.nodeType === 3 || a.nodeType === 8) { if (a.nodeValue !== b.nodeValue) a.nodeValue = b.nodeValue; return; }
    if (a.nodeType !== 1) return;
    if (a.id === 'flyers' || a.nodeName === 'CANVAS') return;
    for (const at of Array.from(a.attributes)) if (!b.hasAttribute(at.name)) a.removeAttribute(at.name);
    for (const at of Array.from(b.attributes)) if (a.getAttribute(at.name) !== at.value) a.setAttribute(at.name, at.value);
    morphChildren(a, b);
  }
  function morphChildren(a, b) {
    const keyed = new Map();
    for (const c of Array.from(a.childNodes)) if (c.nodeType === 1 && c.dataset && c.dataset.key) keyed.set(c.dataset.key, c);
    const next = Array.from(b.childNodes);
    let i = 0;
    for (const nb of next) {
      const k = nb.nodeType === 1 && nb.dataset ? nb.dataset.key : null;
      let old = k ? keyed.get(k) : a.childNodes[i];
      if (k && old) keyed.delete(k);
      if (old && !k && old.nodeType === 1 && old.dataset && old.dataset.key) old = null;
      if (old) {
        if (a.childNodes[i] !== old) a.insertBefore(old, a.childNodes[i] || null);
        morph(old, nb);
      } else {
        if (k && nb.nodeType === 1 && !(nb instanceof SVGElement)) nb.classList.add('enter');
        if (k && nb instanceof SVGElement && nb.classList.contains('cell')) nb.classList.add('born');
        a.insertBefore(nb, a.childNodes[i] || null);
      }
      i++;
    }
    while (a.childNodes.length > i) a.removeChild(a.lastChild);
  }
  function parse(html) { const t = document.createElement('template'); t.innerHTML = html; return t.content; }

  // ---------- render ----------
  function route() {
    const [v, ...rest] = (location.hash.slice(1) || 'home').split('/');
    return { v: views[v] ? v : 'home', arg: decodeURIComponent(rest.join('/')) };
  }
  let renderTimer = 0;
  function render(viaNav) {
    const d = merged();
    if (!d) return;
    const { v, arg } = route();
    const st = derive(d);
    recordTimes(st);
    annotate(st);
    noteTransitions(st);
    syncHive(st);
    const html = views[v](st, arg);
    const swap = shownView !== v;
    const apply = () => {
      if (swap) {
        view.innerHTML = '';
        view.appendChild(parse(html));
        view.classList.add('stagger');
        setTimeout(() => view.classList.remove('stagger'), 900);
        window.scrollTo(0, 0);
      } else {
        const det = $('detail'), keep = det ? det.scrollTop : 0;
        const wrap = document.createElement('div'); wrap.appendChild(parse(html));
        morphChildren(view, wrap);
        const nd = $('detail');
        if (nd && nd === det) nd.scrollTop = keep;
      }
      shownView = v;
      chrome(st, v);
      const hero = $('hero');
      if (hero && hero !== observedHero && heroObserver) {
        if (observedHero) heroObserver.unobserve(observedHero);
        heroObserver.observe(hero);
        observedHero = hero;
      }
      layoutHive();
      runMoves();
    };
    if (swap && viaNav && document.startViewTransition) document.startViewTransition(apply); else apply();
    // Re-render when a "just changed" glow or a landing ends.
    clearTimeout(renderTimer);
    const landing = Math.min(...[...arrive.values()].filter((t) => t > Date.now()), Infinity);
    const at = Math.min(nextExpiry, landing);
    if (isFinite(at)) renderTimer = setTimeout(() => render(false), Math.max(50, at - Date.now() + 30));
  }
  function chrome(st, v) {
    let active = null;
    for (const a of document.querySelectorAll('.item')) {
      if (a.dataset.view === v) { a.setAttribute('aria-current', 'page'); active = a; } else a.removeAttribute('aria-current');
    }
    if (active) $('nav-glow').style.transform = `translate(${active.offsetLeft}px, ${active.offsetTop}px)`;
    const waiting = st.ready.length + st.asks.length;
    const cs = $('c-sessions');
    cs.hidden = !waiting; cs.textContent = waiting; cs.className = 'badge' + (st.asks.length ? ' bad' : '');
    const fails = st.needs.filter((n) => n.tone === 'bad' && n.url).length;
    const cp = $('c-prs');
    cp.hidden = !fails; cp.textContent = fails;
    const cq = $('c-queue'), queued = st.gate ? st.gate.waiting.length : 0;
    if (cq) { cq.hidden = !queued; cq.textContent = queued; }
    document.title = (st.asks.length ? '(?) ' : waiting ? `(${waiting}) ` : '') + 'agentMon';
  }

  // ---------- input: clicks, presses, hover ----------
  function ripple(el, e) {
    if (el instanceof SVGElement || el.classList.contains('cellbox')) return;
    const r = el.getBoundingClientRect();
    const dot = document.createElement('span');
    dot.className = 'ripple';
    dot.style.left = (e.clientX - r.left) + 'px';
    dot.style.top = (e.clientY - r.top) + 'px';
    el.appendChild(dot);
    setTimeout(() => dot.remove(), 650);
  }
  view.addEventListener('click', (e) => {
    const pick = e.target.closest('.pick');
    if (pick) { ripple(pick, e); location.hash = '#projects/' + encodeURIComponent(pick.dataset.name); return; }
    if (e.target.closest('a')) return;
    const f = e.target.closest('[data-pid]');
    if (f && f.dataset.pid && app) {
      ripple(f, e);
      f.classList.add('pressed');
      setTimeout(() => f.classList.remove('pressed'), 380);
      post('focus:' + f.dataset.pid + '|' + (f.dataset.hint || '') + '|' + (f.dataset.sname || ''));
    }
  });
  addEventListener('hashchange', () => render(true));

  // ---------- toast ----------
  let toastTimer = 0;
  function toast(text, action) {
    let t = $('toast');
    if (!t) { t = document.createElement('div'); t.id = 'toast'; t.className = 'toast'; document.body.appendChild(t); }
    t.innerHTML = `<span>${esc(text)}</span>${action ? `<button>${esc(action.label)}</button>` : ''}`;
    if (action) t.querySelector('button').addEventListener('click', action.run);
    t.classList.remove('show'); void t.offsetWidth; t.classList.add('show');
    clearTimeout(toastTimer);
    if (!action) toastTimer = setTimeout(() => t.classList.remove('show'), 4200);
  }
  window.agentMonFocused = (ok) => { if (!ok) toast("Couldn't find that session's window"); };
  let staged = '';
  window.agentMonUpdate = (text, isStaged) => {
    if (isStaged) { staged = text; toast(`agentMon ${text} is ready`, { label: 'Restart to update', run: () => post('update-now') }); } else toast(text);
  };

  // ---------- data in ----------
  // The app pushes both tiers in (agentMonSlow once a minute, agentMonLive on every change); nothing polls.
  window.agentMonLive = (snapshot) => { live = snapshot; render(false); };
  window.agentMonSlow = (data) => { slow = data; slowStamp = data.generated; render(false); };
  function load() {
    const old = $('data-script');
    if (old) old.remove();
    const s = document.createElement('script');
    s.id = 'data-script';
    s.src = 'board-data.js?t=' + Date.now();
    s.onload = () => { if (window.BOARD_DATA && window.BOARD_DATA.generated !== slowStamp) { slow = window.BOARD_DATA; slowStamp = slow.generated; render(false); } };
    document.body.appendChild(s);
  }
  window.agentMonReload = load;
  function tick() {
    $('clock').textContent = new Date().toLocaleTimeString('en-US', { hour: 'numeric', minute: '2-digit' });
    if (!slow) return;
    const age = ms(slow.generated) / 1000, stale = age > 180;
    const f = $('fresh');
    const html = `<span class="beat ${stale ? 'stale' : ''}"></span><span>${stale ? `data is ${Math.round(age / 60)} min old` : live ? 'live' : `updated <b>${esc(ago(slow.generated))}</b>`}</span>`;
    if (f.innerHTML !== html) f.innerHTML = html;
  }

  // ---------- theme + honeycomb background ----------
  const themes = ['auto', 'light', 'dark'];
  function applyTheme() {
    const t = localStorage.getItem('am-theme') || 'auto';
    const dark = t === 'dark' || (t === 'auto' && matchMedia('(prefers-color-scheme: dark)').matches);
    document.documentElement.dataset.theme = dark ? 'dark' : 'light';
    $('theme').title = 'Theme: ' + t + ' (click to change)';
    H.colors = null;
    H.bg = null;
    wakeHive();
  }
  $('theme').addEventListener('click', () => {
    const t = localStorage.getItem('am-theme') || 'auto';
    localStorage.setItem('am-theme', themes[(themes.indexOf(t) + 1) % 3]);
    document.startViewTransition ? document.startViewTransition(applyTheme) : applyTheme();
  });
  matchMedia('(prefers-color-scheme: dark)').addEventListener('change', applyTheme);
  (function comb() {
    const svg = $('comb'), Rr = 34, w = Rr * Math.sqrt(3);
    svg.innerHTML = `<defs><pattern id="hp" width="${w.toFixed(2)}" height="${(Rr * 3).toFixed(2)}" patternUnits="userSpaceOnUse">
      <path d="M${(w / 2).toFixed(2)} 0 L${w.toFixed(2)} ${Rr / 2} L${w.toFixed(2)} ${Rr * 1.5} L${(w / 2).toFixed(2)} ${Rr * 2} L0 ${Rr * 1.5} L0 ${Rr / 2}Z M${(w / 2).toFixed(2)} ${Rr * 2} V${Rr * 3}"/></pattern></defs>
      <rect width="100%" height="100%" fill="url(#hp)" style="stroke:none"/>`;
  })();

  // ---------- desktop widget (agentMon.exe) ----------
  if (app) {
    document.documentElement.classList.add('app');
    let pinned = !!window.AGENTMON_PINNED;
    const pin = $('w-pin');
    const showPin = () => { pin.setAttribute('aria-pressed', pinned); pin.title = pinned ? 'Always on top (click to release)' : 'Keep on top'; };
    const setPinned = (v) => { pinned = v; showPin(); post('pin:' + (v ? 1 : 0)); };
    showPin();
    pin.addEventListener('click', () => setPinned(!pinned));
    $('w-min').addEventListener('click', () => post('min'));
    $('w-close').addEventListener('click', () => post('close'));
    $('grip').addEventListener('mousedown', (e) => { if (e.button === 0) { e.preventDefault(); post('resize:se'); } });
    $('edges').addEventListener('mousedown', (e) => {
      const d = e.target.dataset && e.target.dataset.d;
      if (d && e.button === 0) { e.preventDefault(); e.stopPropagation(); post('resize:' + d); }
    });
    $('topbar').addEventListener('mousedown', (e) => {
      if (e.button === 0 && !e.target.closest('button, a')) { e.preventDefault(); post('drag'); }
    });
    document.addEventListener('click', (e) => {
      const a = e.target.closest('a[target="_blank"]');
      if (a) { e.preventDefault(); post('open:' + a.href); }
    });

    // Right-click menu: always on top, window size, theme, refresh, updates, minimize, close.
    const SIZES = [['Compact', 380, 560], ['Tall', 440, 900], ['Wide', 900, 620], ['Full dashboard', 1400, 900]];
    const menu = document.createElement('div');
    menu.className = 'ctx'; menu.hidden = true; menu.setAttribute('role', 'menu');
    document.body.appendChild(menu);
    const closeMenu = () => { menu.hidden = true; };
    const openMenu = (x, y) => {
      const theme = localStorage.getItem('am-theme') || 'auto';
      const iw = innerWidth, ih = innerHeight;
      const item = (act, label, o) => `<button role="menuitem" data-act="${act}" class="${o && o.on ? 'on' : ''}"><i class="${o && o.check ? 'chk' : ''}"></i><span>${label}</span>${o && o.hint ? `<small>${o.hint}</small>` : ''}</button>`;
      menu.innerHTML =
        item('pin', 'Always on top', { check: true, on: pinned }) +
        '<hr><div class="ctx-h">Window size</div>' +
        SIZES.map(([n, w, h]) => item('size:' + w + 'x' + h, n, { check: true, on: Math.abs(iw - w) < 12 && Math.abs(ih - h) < 12, hint: w + ' × ' + h })).join('') +
        item('resize', 'Resize by dragging', { hint: 'or the corner grip' }) +
        '<hr><div class="ctx-h">Theme</div>' +
        ['auto', 'light', 'dark'].map((t) => item('theme:' + t, t[0].toUpperCase() + t.slice(1), { check: true, on: theme === t })).join('') +
        '<hr>' + item('refresh', 'Refresh now') +
        (staged ? item('update-now', 'Restart to update', { hint: esc(staged) }) : item('update-check', 'Check for updates', { hint: esc(window.AGENTMON_VERSION || '') })) +
        item('min', 'Minimize') + item('close', 'Close agentMon');
      menu.hidden = false;
      const r = menu.getBoundingClientRect();
      menu.style.left = Math.max(4, Math.min(x, iw - r.width - 4)) + 'px';
      menu.style.top = Math.max(4, Math.min(y, ih - r.height - 4)) + 'px';
      menu.classList.remove('pop'); void menu.offsetWidth; menu.classList.add('pop');
    };
    document.addEventListener('contextmenu', (e) => { e.preventDefault(); openMenu(e.clientX, e.clientY); });
    document.addEventListener('mousedown', (e) => { if (!menu.hidden && !menu.contains(e.target)) closeMenu(); }, true);
    addEventListener('keydown', (e) => { if (e.key === 'Escape') closeMenu(); });
    addEventListener('blur', closeMenu);
    menu.addEventListener('click', (e) => {
      const b = e.target.closest('button[data-act]');
      if (!b) return;
      const act = b.dataset.act;
      closeMenu();
      if (act === 'pin') setPinned(!pinned);
      else if (act.startsWith('size:')) post(act);
      else if (act === 'resize') post('resize:se');
      else if (act.startsWith('theme:')) { localStorage.setItem('am-theme', act.slice(6)); applyTheme(); }
      else if (act === 'refresh') { post('refresh'); toast('Refreshing'); }
      else if (act === 'update-check') { post('update-check'); toast('Checking for updates'); }
      else post(act);
    });
  }
  $('fresh').addEventListener('click', () => { if (app) { post('refresh'); toast('Refreshing'); } else load(); });

  applyTheme();
  tick();
  // In the app the data is pushed (ask for the latest once); in a browser, board-data.js is polled.
  if (app) post('ready');
  else { load(); setInterval(load, 15000); }
  setInterval(tick, 1000);
  setInterval(() => render(false), 20000);
  scheduleForager();
  addEventListener('resize', () => { const d = merged(); if (d) chrome(derive(d), shownView); });
})();
