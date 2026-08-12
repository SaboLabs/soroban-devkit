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

  function esc(s) {
    return String(s).replace(/[&<>"']/g, (c) => ({
      '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;', "'": '&#39;',
    }[c]));
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

  function kvTable(rows) {
    const t = document.createElement('table');
    t.className = 'r-table';
    const body = document.createElement('tbody');
    for (const [k, v] of rows) {
      const tr = document.createElement('tr');
      const th = document.createElement('th');
      th.scope = 'row';
      th.textContent = k;
      const td = document.createElement('td');
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

  function render(data) {
    results.innerHTML = '';
    clearError();

    const meta = data.metadata || {};
    const spec = data.spec || null;

    // 1. Metadata
    const metaSec = section('Metadata', '');
    const metaRows = [
      ['File', currentFile ? currentFile.name : '—'],
      ['SHA-256', meta.hash || '—'],
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
      const hashCell = metaTable.querySelectorAll('td')[1];
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
      // Functions
      const fnWrap = document.createElement('div');
      fnWrap.style.padding = '0.7rem 1rem';
      const fnHead = document.createElement('div');
      fnHead.className = 'count';
      fnHead.style.paddingBottom = '0.5rem';
      fnHead.textContent = 'Functions (' + (spec.functions || []).length + ')';
      fnWrap.appendChild(fnHead);
      (spec.functions || []).forEach((f) => {
        const line = document.createElement('div');
        line.className = 'kv';
        line.style.display = 'inline-flex';
        line.style.margin = '0 0.4rem 0.4rem 0';
        const params = (f.parameters || [])
          .map((p) => (p.name ? p.name + ': ' : '') + ((p.type_ && p.type_.name) || '?'))
          .join(', ');
        const outs = (f.outputs || []).map((o) => o.name || '?').join(', ');
        line.innerHTML = '<span class="k">fn</span> ' + esc(f.name) +
          '(' + esc(params) + ')' + (outs ? ' → ' + esc(outs) : '');
        fnWrap.appendChild(line);
      });
      if (!(spec.functions || []).length) {
        fnWrap.appendChild(kvTable([['Functions', 'none']]));
      }
      specSec.appendChild(fnWrap);

      // Custom types
      const types = (spec.custom_types || []);
      const typeSec = document.createElement('div');
      typeSec.style.padding = '0.7rem 1rem';
      const typeHead = document.createElement('div');
      typeHead.className = 'count';
      typeHead.style.paddingBottom = '0.5rem';
      typeHead.textContent = 'Custom Types (' + types.length + ')';
      typeSec.appendChild(typeHead);
      types.forEach((t) => {
        const line = document.createElement('div');
        line.className = 'kv';
        line.style.display = 'inline-flex';
        line.style.margin = '0 0.4rem 0.4rem 0';
        line.innerHTML = '<span class="k">type</span> ' + esc(t.name || '?') +
          ' <span class="k">' + esc(t.kind || '') + '</span>';
        if (t.members && t.members.length) {
          const mem = document.createElement('span');
          mem.className = 'k';
          mem.textContent = ' {' + t.members.map((m) => m.name || '?').join(', ') + '}';
          line.appendChild(mem);
        }
        typeSec.appendChild(line);
      });
      if (!types.length) typeSec.appendChild(kvTable([['Types', 'none']]));
      specSec.appendChild(typeSec);

      // Events
      const events = (spec.events || []);
      const evSec = document.createElement('div');
      evSec.style.padding = '0.7rem 1rem';
      const evHead = document.createElement('div');
      evHead.className = 'count';
      evHead.style.paddingBottom = '0.5rem';
      evHead.textContent = 'Events (' + events.length + ')';
      evSec.appendChild(evHead);
      events.forEach((ev) => {
        const line = document.createElement('div');
        line.className = 'kv';
        line.style.display = 'inline-flex';
        line.style.margin = '0 0.4rem 0.4rem 0';
        line.innerHTML = '<span class="k">event</span> ' + esc(ev.name || '?');
        evSec.appendChild(line);
      });
      if (!events.length) evSec.appendChild(kvTable([['Events', 'none']]));
      specSec.appendChild(evSec);
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