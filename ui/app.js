/* GameVault front end.
 *
 * Plain DOM, no framework: npm is unreachable on this network, and for a handful of
 * list and grid views the framework would not have earned its weight anyway.
 *
 * Everything the user sees is served from the catalogue database, never from a live
 * filesystem read. That is what lets the library stay fully browsable with every drive
 * unplugged — the central promise of the app.
 */

const invoke = window.__TAURI__.core.invoke;

const state = {
  route: 'library',
  drives: [],
  items: [],
  search: '',
  reviewIndex: 0,
  scanning: false,
};

const el = {
  view: document.getElementById('view'),
  title: document.getElementById('page-title'),
  status: document.getElementById('status'),
  search: document.getElementById('search'),
  scanBtn: document.getElementById('scan-btn'),
  reviewCount: document.getElementById('review-count'),
};

/* ------------------------------------------------------------------ utils */

function h(tag, attrs = {}, ...children) {
  const node = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (v === null || v === undefined || v === false) continue;
    if (k === 'class') node.className = v;
    else if (k === 'text') node.textContent = v;
    else if (k.startsWith('on')) node.addEventListener(k.slice(2).toLowerCase(), v);
    else node.setAttribute(k, v === true ? '' : v);
  }
  for (const c of children.flat()) {
    if (c === null || c === undefined || c === false) continue;
    node.append(c.nodeType ? c : document.createTextNode(String(c)));
  }
  return node;
}

function clear(node) {
  while (node.firstChild) node.removeChild(node.firstChild);
}

function setStatus(msg, isError = false) {
  if (!msg) {
    el.status.hidden = true;
    return;
  }
  el.status.hidden = false;
  el.status.className = `status${isError ? ' is-error' : ''}`;
  clear(el.status);
  if (msg === true) {
    el.status.append(h('span', { class: 'spinner' }), t('action.scanning'));
  } else {
    el.status.textContent = msg;
  }
}

/* Paths are shown compactly: the last two segments carry the information a person
 * actually uses to locate something, while the full path stays available on hover.
 * Rendering it in full turned every card into a wall of text. */
function shortPath(p) {
  const parts = String(p).split(/[\\/]/).filter(Boolean);
  if (parts.length <= 2) return parts.join('\\');
  return `…\\${parts.slice(-2).join('\\')}`;
}

/* A stable colour per title, so a game looks the same every session even with no
 * cover art. Hue from a string hash; saturation and lightness fixed so every
 * placeholder sits in the same tonal family as the rest of the UI. */
function coverStyle(title) {
  let hash = 0;
  for (let i = 0; i < title.length; i += 1) {
    hash = (hash * 31 + title.charCodeAt(i)) >>> 0;
  }
  const hue = hash % 360;
  return `background:linear-gradient(150deg,
    hsl(${hue} 42% 32%) 0%,
    hsl(${(hue + 38) % 360} 46% 19%) 100%)`;
}

/* ------------------------------------------------------------------- data */

async function loadDrives() {
  state.drives = await invoke('refresh_drives');
}

async function loadItems(verdict) {
  state.items = await invoke('list_items', {
    filter: {
      drive_id: null,
      verdict: verdict ?? null,
      search: state.search || null,
      limit: 500,
      offset: 0,
    },
  });
}

/* ------------------------------------------------------------------ views */

function emptyState(titleKey, subKey) {
  return h(
    'div',
    { class: 'empty' },
    h('div', { class: 'empty-title', text: t(titleKey) }),
    subKey ? h('div', { text: t(subKey) }) : null,
  );
}

function gameCard(item) {
  const offline = !item.drive_online;
  const parts = item.part_count && item.part_count > 1;

  return h(
    'article',
    { class: 'card', onclick: () => openDetail(item.id) },
    item.cover_path
      ? h('img', {
          class: `cover cover-img${offline ? ' is-offline' : ''}`,
          src: window.__TAURI__.core.convertFileSrc(item.cover_path),
          alt: item.display_title,
          loading: 'lazy',
        })
      : h(
          'div',
          {
            class: `cover${offline ? ' is-offline' : ''}`,
            style: coverStyle(item.display_title),
          },
          item.display_title,
        ),
    h(
      'div',
      { class: 'card-body' },
      h('div', { class: 'card-title', text: item.display_title }),
      h(
        'div',
        { class: 'card-meta' },
        h('span', { text: fmtBytes(item.total_bytes) }),
        h(
          'span',
          { class: `chip ${offline ? 'chip-offline' : 'chip-drive'}` },
          offline ? `${item.drive_label} · ${t('label.offline')}` : item.drive_label,
        ),
        parts
          ? h('span', { class: 'chip chip-parts', text: `${item.part_count} ${t('label.volumes')}` })
          : null,
        item.repacker ? h('span', { class: 'chip', text: item.repacker }) : null,
        item.release_group ? h('span', { class: 'chip', text: item.release_group }) : null,
        item.set_complete === false
          ? h('span', { class: 'chip chip-warn', text: t('msg.incomplete') })
          : null,
      ),
      h('div', { class: 'path', title: item.rel_path, text: shortPath(item.rel_path) }),
    ),
  );
}

async function renderLibrary() {
  await loadItems('game');
  clear(el.view);
  if (state.items.length === 0) {
    el.view.append(emptyState('empty.library', 'empty.librarySub'));
    return;
  }
  const total = state.items.reduce((sum, i) => sum + Number(i.total_bytes), 0);
  el.view.append(
    h(
      'div',
      { class: 'section-head' },
      h('h2', { text: `${fmtNum(state.items.length)} ${t('label.games')}` }),
      h('span', { class: 'count-pill', text: fmtBytes(total) }),
    ),
    h('div', { class: 'grid' }, state.items.map(gameCard)),
  );
}

function capacityBar(drive) {
  // Segments are catalogued games, other catalogued content, and free space. The
  // remainder is whatever the scan did not attribute.
  const total = Number(drive.total_bytes) || 1;
  const games = Number(drive.catalogued_bytes) || 0;
  const free = Number(drive.free_bytes) || 0;
  const other = Math.max(0, total - free - games);
  const pct = (v) => `${Math.max(0, (v / total) * 100)}%`;

  return h(
    'div',
    {},
    h(
      'div',
      { class: 'bar' },
      h('div', { class: 'bar-seg', style: `inline-size:${pct(games)};background:var(--accent)` }),
      h('div', { class: 'bar-seg', style: `inline-size:${pct(other)};background:var(--surface-3)` }),
    ),
    h(
      'div',
      { class: 'legend' },
      h(
        'span',
        { class: 'legend-item' },
        h('span', { class: 'swatch', style: 'background:var(--accent)' }),
        `${t('label.catalogued')} · ${fmtBytes(games)}`,
      ),
      h(
        'span',
        { class: 'legend-item' },
        h('span', { class: 'swatch', style: 'background:var(--surface-3)' }),
        `${t('label.other')} · ${fmtBytes(other)}`,
      ),
      h(
        'span',
        { class: 'legend-item' },
        h('span', { class: 'swatch', style: 'background:var(--line)' }),
        `${t('label.freeSpace')} · ${fmtBytes(free)}`,
      ),
    ),
  );
}

async function renderDrives() {
  await loadDrives();
  clear(el.view);
  if (state.drives.length === 0) {
    el.view.append(emptyState('empty.drives', 'empty.librarySub'));
    return;
  }

  const list = h('div', { class: 'drive-list' });
  for (const d of state.drives) {
    const online = d.is_online;
    const facts = [
      d.filesystem,
      d.bus_type?.toUpperCase(),
      `${fmtBytes(d.free_bytes)} ${t('label.free')} ${t('label.of')} ${fmtBytes(d.total_bytes)}`,
      `${t('label.lastScan')}: ${fmtWhen(d.last_scan_utc)}`,
      `serial ${d.volume_serial}`,
    ].filter(Boolean);

    list.append(
      h(
        'section',
        { class: `drive${online ? '' : ' is-offline'}` },
        h(
          'div',
          { class: 'drive-head' },
          h('span', { class: `dot${online ? '' : ' is-off'}` }),
          h('span', { class: 'drive-name', text: d.label || d.current_mount || d.volume_serial }),
          online && d.current_mount
            ? h('span', { class: 'chip chip-drive', text: d.current_mount })
            : h('span', { class: 'chip chip-offline', text: t('label.offline') }),
          d.is_external ? h('span', { class: 'chip', text: 'USB' }) : null,
          h(
            'span',
            { class: 'count-pill' },
            `${fmtNum(d.game_count)} ${t('label.games')} · ${fmtNum(d.item_count)} ${t('label.items')}`,
          ),
          h(
            'div',
            { class: 'drive-actions' },
            online
              ? h('button', {
                  class: 'btn btn-sm',
                  text: t('action.rescan'),
                  onclick: () => runScan(d.current_mount),
                })
              : null,
          ),
        ),
        h('div', { class: 'drive-sub' }, facts.map((f) => h('span', { text: f }))),
        capacityBar(d),
        d.is_unjournaled
          ? h('div', { class: 'offline-note', text: `⚠ ${t('label.noJournal')}` })
          : null,
        d.max_file_bytes
          ? h('div', {
              class: 'offline-note',
              text: `${t('label.maxFile')}: ${fmtBytes(d.max_file_bytes)}`,
            })
          : null,
        !online ? h('div', { class: 'offline-note', text: t('msg.offlineNote') }) : null,
      ),
    );
  }
  el.view.append(list);
}

async function renderReview() {
  await loadItems('needs_review');
  clear(el.view);
  if (state.items.length === 0) {
    el.view.append(emptyState('empty.review', 'empty.reviewSub'));
    return;
  }

  state.reviewIndex = Math.min(state.reviewIndex, state.items.length - 1);
  const item = state.items[state.reviewIndex];
  let reasons = [];
  try {
    reasons = JSON.parse(item.reasons_json || '[]');
  } catch {
    reasons = [];
  }

  el.view.append(
    h(
      'div',
      { class: 'review-card' },
      h(
        'div',
        { class: 'count-pill', text: `${state.reviewIndex + 1} / ${state.items.length}` },
      ),
      h('h2', { class: 'review-title', text: item.display_title }),
      h('div', { class: 'path', text: item.rel_path }),
      h(
        'div',
        { class: 'card-meta', style: 'margin-block-start:10px' },
        h('span', { class: 'chip', text: t(`kind.${item.kind}`) }),
        h('span', { class: 'chip', text: fmtBytes(item.total_bytes) }),
        h('span', { class: 'chip', text: item.drive_label }),
        h('span', {
          class: 'chip',
          text: `${t('label.confidence')} ${(item.confidence * 100).toFixed(0)}%`,
        }),
      ),
      h(
        'div',
        { class: 'reasons' },
        reasons.length
          ? reasons.map((r) =>
              h(
                'div',
                { class: 'reason' },
                h('span', {
                  class: `reason-delta ${r.delta >= 0 ? 'pos' : 'neg'}`,
                  text: `${r.delta >= 0 ? '+' : ''}${r.delta.toFixed(2)}`,
                }),
                h('span', { text: r.note }),
              ),
            )
          : h('div', { class: 'reason', text: '—' }),
      ),
      h(
        'div',
        { class: 'review-actions' },
        h(
          'button',
          { class: 'btn btn-primary', onclick: () => decide(item.id, 'game') },
          t('action.isGame'),
          h('span', { class: 'kbd', text: 'G' }),
        ),
        h(
          'button',
          { class: 'btn', onclick: () => decide(item.id, 'not_game') },
          t('action.notGame'),
          h('span', { class: 'kbd', text: 'X' }),
        ),
        h(
          'button',
          { class: 'btn btn-ghost', onclick: () => skipReview() },
          t('action.skip'),
          h('span', { class: 'kbd', text: '→' }),
        ),
      ),
    ),
  );
}

async function renderFiltered() {
  await loadItems('not_game');
  clear(el.view);
  if (state.items.length === 0) {
    el.view.append(emptyState('empty.filtered'));
    return;
  }

  const table = h(
    'table',
    { class: 'table' },
    h(
      'thead',
      {},
      h(
        'tr',
        {},
        h('th', { text: t('title.library') }),
        h('th', { text: t('nav.storage') }),
        h('th', { text: '' }),
        h('th', { text: '' }),
      ),
    ),
    h(
      'tbody',
      {},
      state.items.map((i) => {
        let why = '';
        try {
          why = (JSON.parse(i.reasons_json || '[]')[0] || {}).note || '';
        } catch {
          why = '';
        }
        return h(
          'tr',
          {},
          h(
            'td',
            {},
            h('div', { text: i.display_title }),
            h('div', { class: 'path', title: i.rel_path, text: shortPath(i.rel_path) }),
          ),
          h('td', { class: 'num', text: fmtBytes(i.total_bytes) }),
          h('td', {}, h('span', { class: 'chip', text: t(`kind.${i.kind}`) }), ' ', why),
          h(
            'td',
            { class: 'num' },
            h('button', {
              class: 'btn btn-sm',
              text: t('action.restore'),
              onclick: () => decide(i.id, 'game'),
            }),
          ),
        );
      }),
    ),
  );

  el.view.append(h('div', { class: 'offline-note', text: t('msg.filteredNote') }), table);
}

async function renderDuplicates() {
  const groups = await invoke('list_duplicates');
  clear(el.view);
  if (groups.length === 0) {
    el.view.append(emptyState('empty.duplicates', 'empty.duplicatesSub'));
    return;
  }

  for (const g of groups) {
    // Reclaimable = everything beyond the one copy you keep.
    const each = Number(g.total_bytes) / Math.max(1, Number(g.copies));
    const reclaimable = Number(g.total_bytes) - each;
    el.view.append(
      h(
        'div',
        { class: 'dup-group' },
        h(
          'div',
          { class: 'section-head' },
          h('h2', { text: g.items[0]?.display_title || g.title_key }),
          h('span', {
            class: 'chip chip-warn',
            text: `${g.copies} ${t('label.copies')}`,
          }),
          h('span', {
            class: 'count-pill',
            text: `${t('label.reclaimable')} ${fmtBytes(reclaimable)}`,
          }),
        ),
        h(
          'table',
          { class: 'table' },
          h(
            'tbody',
            {},
            g.items.map((i) =>
              h(
                'tr',
                {},
                h(
                  'td',
                  {},
                  h('span', {
                    class: `chip ${i.drive_online ? 'chip-drive' : 'chip-offline'}`,
                    text: i.drive_online ? i.drive_label : `${i.drive_label} · ${t('label.offline')}`,
                  }),
                ),
                h('td', {}, h('span', { class: 'path', title: i.rel_path, text: shortPath(i.rel_path) })),
                h('td', { class: 'num', text: fmtBytes(i.total_bytes) }),
              ),
            ),
          ),
        ),
      ),
    );
  }
}

async function renderStorage() {
  await loadDrives();
  clear(el.view);
  if (state.drives.length === 0) {
    el.view.append(emptyState('empty.drives'));
    return;
  }

  for (const d of state.drives) {
    const slices = await invoke('storage_breakdown', { driveId: d.id });
    el.view.append(
      h(
        'div',
        { class: 'dup-group' },
        h(
          'div',
          { class: 'section-head' },
          h('h2', { text: d.label || d.volume_serial }),
          h('span', {
            class: `chip ${d.is_online ? 'chip-drive' : 'chip-offline'}`,
            text: d.is_online ? t('label.online') : t('label.offline'),
          }),
        ),
        slices.length
          ? h(
              'table',
              { class: 'table' },
              h(
                'tbody',
                {},
                slices.map((s) =>
                  h(
                    'tr',
                    {},
                    h('td', {}, h('span', { class: 'chip', text: t(`kind.${s.kind}`) })),
                    h('td', { class: 'num', text: `${fmtNum(s.count)} ${t('label.items')}` }),
                    h('td', { class: 'num', text: fmtBytes(s.bytes) }),
                  ),
                ),
              ),
            )
          : h('div', { class: 'empty', text: t('label.never') }),
      ),
    );
  }
}

/* ---------------------------------------------------------------- actions */

async function decide(itemId, verdict) {
  await invoke('set_verdict', { itemId, verdict });
  await render();
  await updateReviewBadge();
}

function skipReview() {
  state.reviewIndex += 1;
  if (state.reviewIndex >= state.items.length) state.reviewIndex = 0;
  render();
}

/* Scan a drive. With one drive attached it just runs; with several the user picks. */
async function runScan(mount) {
  if (state.scanning) return;

  if (!mount) {
    await loadDrives();
    const online = state.drives.filter((d) => d.is_online && d.current_mount);
    if (online.length === 0) {
      setStatus(t('empty.drives'), true);
      return;
    }
    if (online.length === 1) {
      mount = online[0].current_mount;
    } else {
      showDrivePicker(online);
      return;
    }
  }

  state.scanning = true;
  el.scanBtn.disabled = true;
  setStatus(true);
  try {
    const s = await invoke('scan_path', { path: mount });
    setStatus(
      `${t('msg.scanDone')} — ${s.drive_label}: ${fmtNum(s.items_found)} ${t('msg.found')}, ` +
        `${fmtNum(s.items_new)} ${t('msg.new')}, ${fmtNum(s.items_missing)} ${t('msg.missing')} · ` +
        `${fmtNum(s.files_seen)} ${t('label.items')} ${t('msg.in')} ${s.elapsed_ms} ms`,
    );
  } catch (e) {
    setStatus(e?.message || String(e), true);
  } finally {
    state.scanning = false;
    el.scanBtn.disabled = false;
    await render();
    await updateReviewBadge();
  }
}

function showDrivePicker(drives) {
  clear(el.view);
  el.view.append(
    h(
      'div',
      { class: 'review-card' },
      h('h2', { class: 'review-title', text: t('msg.pickDrive') }),
      h(
        'div',
        { class: 'review-actions' },
        drives.map((d) =>
          h('button', {
            class: 'btn',
            text: `${d.current_mount}  ${d.label || ''}`.trim(),
            onclick: () => runScan(d.current_mount),
          }),
        ),
      ),
    ),
  );
}

async function updateReviewBadge() {
  try {
    const pending = await invoke('list_items', {
      filter: { drive_id: null, verdict: 'needs_review', search: null, limit: 500, offset: 0 },
    });
    el.reviewCount.hidden = pending.length === 0;
    el.reviewCount.textContent = String(pending.length);
  } catch {
    el.reviewCount.hidden = true;
  }
}

/* ---------------------------------------------------------------- routing */

const ROUTES = {
  library: renderLibrary,
  drives: renderDrives,
  review: renderReview,
  filtered: renderFiltered,
  transfers: renderTransfers,
  duplicates: renderDuplicates,
  storage: renderStorage,
  settings: renderSettings,
};

async function render() {
  el.title.textContent = t(`title.${state.route}`);
  // The search box only applies to list views.
  el.search.parentElement.hidden = !['library', 'filtered'].includes(state.route);
  try {
    await ROUTES[state.route]();
  } catch (e) {
    clear(el.view);
    el.view.append(h('div', { class: 'empty' }, h('div', { class: 'empty-title', text: String(e?.message || e) })));
  }
}

function go(route) {
  state.route = route;
  state.reviewIndex = 0;
  document.querySelectorAll('.nav-item').forEach((b) => {
    b.classList.toggle('is-active', b.dataset.route === route);
  });
  render();
}

/* -------------------------------------------------------------------- init */

document.querySelectorAll('.nav-item').forEach((b) => {
  b.addEventListener('click', () => go(b.dataset.route));
});

el.scanBtn.addEventListener('click', () => runScan(null));

document.getElementById('lang-toggle').addEventListener('click', () => {
  setLang(currentLang() === 'ar' ? 'en' : 'ar');
  render();
});

let searchTimer = null;
el.search.addEventListener('input', (e) => {
  state.search = e.target.value.trim();
  clearTimeout(searchTimer);
  searchTimer = setTimeout(render, 180);
});

// Keyboard-first review: the queue is meant to be cleared quickly.
document.addEventListener('keydown', (e) => {
  if (state.route !== 'review' || e.target.tagName === 'INPUT') return;
  const item = state.items[state.reviewIndex];
  if (!item) return;
  const key = e.key.toLowerCase();
  if (key === 'g') decide(item.id, 'game');
  else if (key === 'x') decide(item.id, 'not_game');
  else if (e.key === 'ArrowRight' || e.key === 'ArrowLeft') skipReview();
});

(async function init() {
  loadLang();
  initTransferEvents();
  initMetaEvents();
  await loadDrives();
  await updateReviewBadge();
  go('library');
})();
