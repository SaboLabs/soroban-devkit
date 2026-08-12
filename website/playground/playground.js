/* Soroban DevKit Web Playground — main thread.
 *
 * Responsibilities:
 *   - file picker + dropzone (client-side validation only)
 *   - hand bytes to the worker (inspection runs off the UI thread)
 *   - render the structured result returned by the Rust inspector
 *   - reset / re-upload
 *
 * The uploaded bytes exist only as a Uint8Array on this thread; they are
 * transferred into the worker, inspected there, and never sent anywhere.
 * The only network requests the app makes are for its own static assets
 * (this page, the CSS, the worker, and the bundled sdkt WASM runtime).
 */
'use strict';

(function () {
  const worker = new Worker('worker.js', { type: 'module' });

  const $ = (id) => document.getElementById(id);
  const dropzone = $('dropzone');
  const fileInput = $('fileInput');
  const filebar = $('filebar');
  const fname = $('fname');
  const fmeta = $('fmeta');
  const status = $('status');
  const errorBox = $('errorBox');
  const errorMsg = $('errorMsg');
  const results = $('results');
  const resetBtn = $('resetBtn');
  const modeChip = $('modeChip');

  // Future network capability placeholder. Group 2 (RPC reads) and Group 3
  // (transactions) will plug in here; the local inspector never touches it.
  const networkProvider = { mode: 'local', supported: ['local'] };

  let nextId = 1;
  const pending = new Map();
  let currentFile = null;
  let inspected = false;

  /* ---------- helpers ---------- */

  function fmtSize(bytes) {
    if (bytes < 1024) return bytes + ' B';
    if (bytes < 1048576) return (bytes / 1024).toFixed(1) + ' KiB';
    return (bytes / 1048576).toFixed(2) + ' MiB';
  }

  function setStatus(kind, text) {
    status.className = 'status';
    if (kind) status.classList.add(kind);
    status.textContent = text || '';
    if (kind === 'loading') {
      const s = document.createElement('span');
      s.className = 'spinner';
      s.setAttribute('aria-hidden', 'true');
      status.prepend(s);
    }
  }

  function showError(msg) {
    errorMsg.textContent = msg;
    errorBox.hidden = false;
    results.classList.add('hidden');
    inspected = false;
  }

  function clearError() { errorBox.hidden = true; }

  function post(msg) {
    return new Promise((resolve, reject) => {
      const id = nextId++;
      pending.set(id, { resolve, reject });
      worker.postMessage(Object.assign({ id }, msg));
    });
  }

  function copyBtn(value) {
    const b = document.createElement('button');
    b.type = 'button';
    b.className = 'copy-btn';
    b.textContent = 'copy';
    b.addEventListener('click', () => {
      navigator.clipboard.writeText(value).then(() => {
        b.textContent = 'copied';
        setTimeout(() => { b.textContent = 'copy'; }, 1200);
      }).catch(() => { b.textContent = 'copy'; });
    });
    return b;
  }

  /* ---------- rendering ---------- */

  function section(title, countLabel) {
    const sec = document.createElement('section');
    sec.className = 'result-section';
    const h = document.createElement('h2');
    h.textContent = title;
    if (countLabel) {
      const c = document.createElement('span');
      c.className = 'count';
      c.textContent = countLabel;
      h.appendChild(c);
    }
    sec.appendChild(h);
    return sec;
  }

  // rows: [label, value] or [label, value, valueClass]. `valueClass` lets a
  // single cell opt into different wrapping (the SHA-256 digest).
  function kvTable(rows) {
    const t = document.createElement('table');
    t.className = 'r-table';
    const body = document.createElement('tbody');
    for (const [k, v, cls] of rows) {
      const tr = document.createElement('tr');
      const th = document.createElement('th');
      th.scope = 'row';
      th.textContent = k;
      const td = document.createElement('td');
      if (cls) td.className = cls;
      td.textContent = v;
      tr.appendChild(th); tr.appendChild(td);
      body.appendChild(tr);
    }
    t.appendChild(body);
    return t;
  }

  function kvChips(items) {
    const wrap = document.createElement('div');
    wrap.className = 'kv-row';
    for (const { k, v } of items) {
      const chip = document.createElement('span');
      chip.className = 'kv';
      const key = document.createElement('span');
      key.className = 'k';
      key.textContent = k + ': ';
      chip.appendChild(key);
      chip.appendChild(document.createTextNode(v));
      wrap.appendChild(chip);
    }
    return wrap;
  }

  /* ---------- contract-spec rendering ---------- */

  // A titled group inside the Contract Specification section. The count uses the
  // same `.count` badge treatment as the top-level section headings, so the two
  // heading levels read consistently.
  function specGroup(title, count) {
    const g = document.createElement('div');
    g.className = 'spec-group';
    const h = document.createElement('h3');
    h.textContent = title;
    const c = document.createElement('span');
    c.className = 'count';
    c.textContent = count + (count === 1 ? ' item' : ' items');
    h.appendChild(c);
    g.appendChild(h);
    return g;
  }

  function specNone(label) {
    const p = document.createElement('p');
    p.className = 'spec-none';
    p.textContent = label;
    return p;
  }

  // One "name: type" row. `meta` mutes the value (used for return types).
  function paramRow(name, value, meta) {
    const row = document.createElement('div');
    row.className = 'param' + (meta ? ' is-meta' : '');
    const dt = document.createElement('dt');
    dt.textContent = name;
    const dd = document.createElement('dd');
    dd.textContent = value;
    row.appendChild(dt);
    row.appendChild(dd);
    return row;
  }

  function specDoc(text) {
    const p = document.createElement('p');
    p.className = 'spec-doc';
    p.textContent = text;
    return p;
  }

  // <article> per entry: bold name, optional kind badge, then detail rows.
  function specItem(name, kind) {
    const item = document.createElement('article');
    item.className = 'spec-item';
    const head = document.createElement('div');
    const strong = document.createElement('strong');
    strong.className = 'spec-name';
    strong.textContent = name || '?';
    head.appendChild(strong);
    if (kind) {
      // Explicit whitespace so the name and kind stay separate for text
      // extraction / screen readers, not just visually via margin.
      head.appendChild(document.createTextNode(' '));
      const k = document.createElement('span');
      k.className = 'spec-kind';
      k.textContent = kind;
      head.appendChild(k);
    }
    item.appendChild(head);
    return item;
  }

  function specList() {
    const l = document.createElement('div');
    l.className = 'spec-list';
    return l;
  }

  // An entry with no doc, parameters, or members carries no detail rows, so a
  // full bordered card is pure chrome. Mark it for the flat treatment.
  function markIfBare(item) {
    if (item.childElementCount <= 1) item.classList.add('is-bare');
    return item;
  }

  function renderFunctions(fns) {
    const g = specGroup('Functions', fns.length);
    if (!fns.length) { g.appendChild(specNone('none')); return g; }
    const list = specList();
    fns.forEach((f) => {
      const item = specItem(f.name);
      if (f.doc) item.appendChild(specDoc(f.doc));
      const params = f.parameters || [];
      const outputs = f.outputs || [];
      if (params.length || outputs.length) {
        const dl = document.createElement('dl');
        dl.className = 'params';
        params.forEach((p) => {
          dl.appendChild(paramRow(
            p.name || '?',
            (p.type_ && p.type_.name) || '?',
            false
          ));
          if (p.doc) {
            // Indented under its own parameter so ownership is unambiguous.
            const d = specDoc(p.doc);
            d.classList.add('param-doc');
            dl.appendChild(d);
          }
        });
        outputs.forEach((o) => {
          dl.appendChild(paramRow('returns', o.name || '?', true));
        });
        item.appendChild(dl);
      }
      list.appendChild(markIfBare(item));
    });
    g.appendChild(list);
    return g;
  }

  function renderTypes(types) {
    const g = specGroup('Custom Types', types.length);
    if (!types.length) { g.appendChild(specNone('none')); return g; }
    const list = specList();
    types.forEach((t) => {
      const item = specItem(t.name, t.kind || '');
      if (t.doc) item.appendChild(specDoc(t.doc));
      const members = t.members || [];
      if (members.length) {
        const ul = document.createElement('ul');
        ul.className = 'params spec-members';
        members.forEach((m) => {
          const li = document.createElement('li');
          li.textContent = m.name || '?';
          if (m.doc) li.title = m.doc;
          ul.appendChild(li);
        });
        item.appendChild(ul);
      }
      list.appendChild(markIfBare(item));
    });
    g.appendChild(list);
    return g;
  }

  function renderEvents(events) {
    const g = specGroup('Events', events.length);
    if (!events.length) { g.appendChild(specNone('none')); return g; }
    const list = specList();
    events.forEach((ev) => {
      const item = specItem(ev.name);
      if (ev.doc) item.appendChild(specDoc(ev.doc));
      list.appendChild(markIfBare(item));
    });
    g.appendChild(list);
    return g;
  }

  function render(data) {
    results.innerHTML = '';
    clearError();

    const meta = data.metadata || {};
    const spec = data.spec || null;

    // 1. Metadata
    const metaSec = section('Metadata', '');
    const metaRows = [
      ['File', currentFile ? currentFile.name : '—'],
      ['SHA-256', meta.hash || '—', 'hash'],
      ['Size', (meta.size_bytes !== undefined ? fmtSize(meta.size_bytes) : '—') +
        (meta.size_bytes !== undefined ? ' (' + meta.size_bytes + ' bytes)' : '')],
      ['Version', meta.version !== undefined ? String(meta.version) : '—'],
    ];
    if (spec && spec.env_meta && spec.env_meta.interface_version !== undefined) {
      metaRows.push(['Interface version', String(spec.env_meta.interface_version)]);
    }
    if (data.duration_ms !== undefined) {
      metaRows.push(['Inspection time', data.duration_ms + ' ms']);
    }
    const metaTable = kvTable(metaRows);
    // Copy button for the hash (the one value developers actually copy).
    if (meta.hash) {
      const hashCell = metaTable.querySelector('td.hash');
      if (hashCell) hashCell.appendChild(copyBtn(meta.hash));
    }
    metaSec.appendChild(metaTable);
    results.appendChild(metaSec);

    // 2. Exports
    const exps = (meta.exports || []).slice();
    const expSec = section('Exports', exps.length + (exps.length === 1 ? ' item' : ' items'));
    if (exps.length) {
      expSec.appendChild(kvChips(exps.map((e) => ({ k: e.kind, v: e.name }))));
    } else {
      expSec.appendChild(kvTable([['Exports', 'none']]));
    }
    results.appendChild(expSec);

    // 3. Imports
    const imps = (meta.imports || []).slice();
    const impSec = section('Imports', imps.length + (imps.length === 1 ? ' item' : ' items'));
    if (imps.length) {
      impSec.appendChild(kvChips(imps.map((i) => ({ k: i.module, v: i.name + ' [' + i.kind + ']' }))));
    } else {
      impSec.appendChild(kvTable([['Imports', 'none']]));
    }
    results.appendChild(impSec);

    // 4. Custom sections
    const customs = (meta.custom_sections || []).slice();
    const csSec = section('Custom Sections', customs.length + (customs.length === 1 ? ' item' : ' items'));
    if (customs.length) {
      csSec.appendChild(kvChips(customs.map((n) => ({ k: 'name', v: n }))));
    } else {
      csSec.appendChild(kvTable([['Sections', 'none']]));
    }
    results.appendChild(csSec);

    // 5. Contract specification (optional)
    const specSec = section('Contract Specification', spec ? 'available' : 'not present');
    if (spec) {
      specSec.appendChild(renderFunctions(spec.functions || []));
      specSec.appendChild(renderTypes(spec.custom_types || []));
      specSec.appendChild(renderEvents(spec.events || []));
    } else {
      const note = document.createElement('div');
      note.style.padding = '0.7rem 1rem';
      const p = document.createElement('p');
      p.style.color = 'var(--muted)';
      p.style.fontSize = '0.84rem';
      p.style.margin = '0';
      p.textContent = data.spec_error || 'No contract spec was produced by the inspector.';
      note.appendChild(p);
      specSec.appendChild(note);
    }
    results.appendChild(specSec);

    results.classList.remove('hidden');
    inspected = true;
  }

  /* ---------- file flow ---------- */

  function acceptFile(file) {
    if (!file) return;
    // Extension + content validation happens in inspect() after read; the
    // picker is restricted to .wasm already, but drops can bypass that.
    currentFile = file;
    filebar.hidden = false;
    fname.textContent = file.name;
    fmeta.textContent = fmtSize(file.size) + ' · ' + (file.type || 'application/octet-stream');
    clearError();
    results.classList.add('hidden');
    inspected = false;
    setStatus('loading', 'inspecting…');
    inspectFile(file);
  }

  function inspectFile(file) {
    file.arrayBuffer().then((buf) => {
      const bytes = new Uint8Array(buf);
      return post({ type: 'inspect', bytes });
    }).then((result) => {
      const dur = (result && result.duration_ms !== undefined)
        ? ' · ' + result.duration_ms + ' ms' : '';
      setStatus('ok', 'inspection complete' + dur);
      render(result);
    }).catch((err) => {
      setStatus('err', 'inspection failed');
      showError(String(err && err.message ? err.message : err));
    });
  }

  function reset() {
    currentFile = null;
    inspected = false;
    fileInput.value = '';
    filebar.hidden = true;
    results.classList.add('hidden');
    clearError();
    setStatus('', '');
    dropzone.focus();
  }

  /* ---------- events ---------- */

  fileInput.addEventListener('change', () => {
    if (fileInput.files && fileInput.files[0]) acceptFile(fileInput.files[0]);
  });

  ['dragenter', 'dragover'].forEach((evt) =>
    dropzone.addEventListener(evt, (e) => { e.preventDefault(); dropzone.classList.add('drag'); }));

  ['dragleave', 'drop'].forEach((evt) =>
    dropzone.addEventListener(evt, (e) => { e.preventDefault(); dropzone.classList.remove('drag'); }));

  dropzone.addEventListener('drop', (e) => {
    e.preventDefault();
    const files = e.dataTransfer && e.dataTransfer.files;
    if (files && files[0]) acceptFile(files[0]);
  });

  dropzone.addEventListener('keydown', (e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      fileInput.click();
    }
  });

  resetBtn.addEventListener('click', reset);

  /* ---------- worker responses ---------- */

  worker.onmessage = (e) => {
    const msg = e.data || {};
    const id = msg.id;
    if (id === 'boot') {
      if (msg.ok) {
        modeChip.textContent = 'Local analysis — no RPC required';
      } else {
        // WASM runtime failed to load; keep the UI usable with a clear note.
        modeChip.textContent = 'WASM runtime unavailable';
        setStatus('err', 'WASM runtime failed to load');
        showError('The WASM runtime could not be initialised. Reload the page or check that the playground assets (wasm/*.wasm) are deployed next to this page.');
      }
      return;
    }
    const p = pending.get(id);
    if (!p) return;
    pending.delete(id);
    if (msg.ok) p.resolve(msg.payload);
    else p.reject(new Error(msg.error));
  };

  worker.onerror = (e) => {
    e.preventDefault();
    if (!pending.size) return;
    // Surface as a rejection so a hung/broken worker does not leave the UI
    // spinning forever.
    for (const [, p] of pending) p.reject(new Error('Worker error: ' + (e.message || 'unknown')));
    pending.clear();
    setStatus('err', 'worker error');
  };

  /* ---------- init ---------- */

  // Warm up WASM in the background so the first inspect is fast.
  post({ type: 'ping' }).catch(() => {});
})();