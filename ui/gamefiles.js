/* ملفات الألعاب — the Game Files screen.
 *
 * The one screen that reads the filesystem instead of the catalogue. That is deliberate:
 * a package copied onto the drive a minute ago should simply be there when this opens, and
 * one deleted should be gone, with no scan in between. The backend keeps a probe cache so
 * only the first read of a drive is slow.
 *
 * Covers come out of the game files themselves — `icon0.png` inside a PS4 package — and
 * fall back to an image sitting beside the file, then to whatever the metadata fetch found,
 * then to a coloured gradient. A card is never empty.
 *
 * Loaded before app.js, so nothing here may touch app.js globals at parse time. Calling
 * `h`, `t`, `invoke` or reading `state` inside a function is fine; that happens at click
 * and render time, exactly as settings.js already does.
 */

const gfState = {
  /** Folder being read. Remembered across sessions. */
  root: null,
  platform: null,
  view: 'grouped',
  /** The last `ConsoleRead` from the backend. */
  data: null,
  reading: false,
  progress: null,
  error: null,
};

const GF_ROOT_KEY = 'gv.gamefiles.root';

function gfLoadRoot() {
  try {
    gfState.root = localStorage.getItem(GF_ROOT_KEY) || null;
  } catch {
    gfState.root = null;
  }
}

function gfSaveRoot(root) {
  gfState.root = root;
  try {
    if (root) localStorage.setItem(GF_ROOT_KEY, root);
    else localStorage.removeItem(GF_ROOT_KEY);
  } catch {
    /* Private mode or blocked storage: the folder just is not remembered. */
  }
}

/* ------------------------------------------------------------------ reading */

async function gfRead(root) {
  if (gfState.reading) return;
  gfSaveRoot(root);
  gfState.reading = true;
  gfState.error = null;
  gfState.progress = null;
  renderGameFiles();

  try {
    gfState.data = await invoke('read_console_dir', { root });
  } catch (e) {
    gfState.error = String(e?.message || e);
    gfState.data = null;
  } finally {
    gfState.reading = false;
    gfState.progress = null;
    if (state.route === 'gamefiles') renderGameFiles();
  }
}

/* The read runs on a background thread; these events keep the status line moving during
 * the first read of a drive, which is the only slow one. */
function initConsoleEvents() {
  window.__TAURI__.event.listen('console://progress', ({ payload }) => {
    if (!gfState.reading || payload.finished) return;
    gfState.progress = payload;
    if (state.route === 'gamefiles') gfUpdateProgress();
  });
}

/* ------------------------------------------------------------------- pieces */

/* Covers are served over the asset protocol from the local cache. If that request is
 * blocked or the cached file has been deleted, fall back to the gradient rather than
 * showing a broken image. */
function gfCover(g) {
  const gradient = () =>
    h('div', { class: 'cover', style: coverStyle(g.display_title) }, g.display_title);

  if (!g.cover_path) return gradient();

  const img = h('img', {
    class: 'cover cover-img',
    src: window.__TAURI__.core.convertFileSrc(g.cover_path),
    alt: g.display_title,
    loading: 'lazy',
    onerror: () => img.replaceWith(gradient()),
  });
  return img;
}

function gfRoleChips(g) {
  return g.roles.map((r) =>
    h('span', { class: 'chip chip-role', text: `${fmtNum(r.count)} ${t(`role.${r.role}`)}` }),
  );
}

function consoleCard(g) {
  const fromFile = g.cover_source === 'pkg_icon0';

  return h(
    'article',
    { class: 'card', onclick: () => gfOpenDetail(g) },
    h(
      'div',
      { class: 'gf-cover-wrap' },
      gfCover(g),
      fromFile ? h('span', { class: 'cover-badge', text: t('label.coverFromFile') }) : null,
    ),
    h(
      'div',
      { class: 'card-body' },
      h('div', { class: 'card-title', title: g.display_title, text: g.display_title }),
      h(
        'div',
        { class: 'card-meta' },
        h('span', { class: 'chip chip-platform', text: t(`platform.${g.platform}`) }),
        // A disc image has no id to show: nothing has walked the filesystem inside it.
        g.title_id ? h('span', { class: 'chip chip-titleid', text: g.title_id }) : null,
        // Say so when the console was inferred from the folder rather than read from the
        // file. A guess presented as a fact is worse than a guess labelled as one.
        g.platform_guessed
          ? h('span', {
              class: 'chip chip-guess',
              title: t('msg.platformGuessed'),
              text: t('label.guessed'),
            })
          : null,
        h('span', { text: fmtBytes(g.total_bytes) }),
        g.firmware_label
          ? h('span', { class: 'chip chip-fw', text: `${t('label.firmware')} ${g.firmware_label}` })
          : null,
      ),
      h('div', { class: 'card-meta' }, gfRoleChips(g)),
      g.base_title_id
        ? h('div', {
            class: 'chip chip-warn',
            title: g.link_reason || '',
            text: `${t('label.addonFor')} ${g.base_title_id}`,
          })
        : null,
      g.rel_dir
        ? h('div', { class: 'path', title: g.rel_path, text: shortPath(g.rel_path) })
        : null,
    ),
  );
}

/* Every file of one game, so a split part or a damaged member is visible rather than
 * hidden inside a total. */
function gfOpenDetail(g) {
  modal(
    g.display_title,
    h(
      'div',
      {},
      h(
        'dl',
        { class: 'kv' },
        g.title_id ? h('dt', { text: t('label.titleId') }) : null,
        g.title_id ? h('dd', {}, h('span', { class: 'path', text: g.title_id })) : null,
        h('dt', { text: t('label.platform') }),
        h('dd', {
          text: g.platform_guessed
            ? `${t(`platform.${g.platform}`)} — ${t('msg.platformGuessed')}`
            : t(`platform.${g.platform}`),
        }),
        // Whether the name is the game's own or came off the filename. Worth stating:
        // several filenames on this drive name the wrong game entirely.
        h('dt', { text: t('label.titleSource') }),
        h('dd', { text: g.title_from_file ? t('title.fromFile') : t('title.fromFilename') }),
        g.firmware_label ? h('dt', { text: t('label.firmware') }) : null,
        g.firmware_label ? h('dd', {}, h('span', { class: 'path', text: g.firmware_label })) : null,
        h('dt', { text: t('label.size') }),
        h('dd', { text: fmtBytes(g.total_bytes) }),
        h('dt', { text: t('label.coverSource') }),
        h('dd', { text: g.cover_source ? t(`cover.${g.cover_source}`) : t('cover.none') }),
        h('dt', { text: t('label.folder') }),
        h('dd', {}, h('span', { class: 'path', text: g.rel_dir || '\\' })),
      ),
      g.link_reason
        ? h('div', { class: 'notice notice-info', text: g.link_reason })
        : null,
      h('div', { class: 'file-list' }, g.files.map(gfFileRow)),
    ),
  );
}

function gfFileRow(f) {
  return h(
    'div',
    { class: 'file-row' },
    f.part_index ? h('span', { class: 'part-no', text: String(f.part_index) }) : null,
    h('span', { class: 'path', title: f.name, text: f.name }),
    h('span', { class: 'chip chip-role', text: t(`role.${f.role}`) }),
    f.app_ver ? h('span', { class: 'chip', text: f.app_ver }) : null,
    h('span', { text: fmtBytes(f.size_bytes) }),
  );
}

/* --------------------------------------------------------------- the toolbar */

function gfToolbar() {
  // External drives first: those are the game drives, and offering the system drive at the
  // head of the row invites a very long read of somewhere with no games in it.
  const online = state.drives
    .filter((d) => d.is_online && d.current_mount)
    .sort((a, b) => Number(b.is_external) - Number(a.is_external));
  const pathInput = h('input', {
    type: 'text',
    value: gfState.root || '',
    placeholder: t('label.folderPath'),
    dir: 'ltr',
  });

  const readBtn = h('button', {
    class: 'btn btn-primary',
    text: t('action.readFolder'),
    disabled: gfState.reading,
    onclick: () => {
      const v = pathInput.value.trim();
      if (v) gfRead(v);
    },
  });
  pathInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') readBtn.click();
  });

  return h(
    'div',
    { class: 'gf-toolbar' },
    // No folder picker exists in this build, so the drives themselves are the shortcut
    // and the field covers any folder inside one.
    online.map((d) =>
      h('button', {
        class: `btn${gfState.root === d.current_mount ? ' btn-primary' : ''}`,
        text: `${d.current_mount} ${d.label || ''}`.trim(),
        disabled: gfState.reading,
        onclick: () => gfRead(d.current_mount),
      }),
    ),
    h('label', { class: 'gf-path' }, pathInput),
    readBtn,
  );
}

function gfFilters(games) {
  const counts = new Map();
  for (const g of games) counts.set(g.platform, (counts.get(g.platform) || 0) + 1);
  const platforms = [...counts.keys()].sort();

  const pick = (p) => () => {
    gfState.platform = p;
    renderGameFiles();
  };

  // Only worth a filter once there is more than one platform on the drive.
  const platformSeg =
    platforms.length > 1
      ? h(
          'div',
          { class: 'seg' },
          h('button', {
            class: gfState.platform === null ? 'is-active' : '',
            text: `${t('filter.allPlatforms')} (${fmtNum(games.length)})`,
            onclick: pick(null),
          }),
          platforms.map((p) =>
            h('button', {
              class: gfState.platform === p ? 'is-active' : '',
              text: `${t(`platform.${p}`)} (${fmtNum(counts.get(p))})`,
              onclick: pick(p),
            }),
          ),
        )
      : null;

  const setView = (v) => () => {
    gfState.view = v;
    renderGameFiles();
  };

  return h(
    'div',
    { class: 'gf-toolbar' },
    platformSeg,
    h(
      'div',
      { class: 'seg' },
      h('button', {
        class: gfState.view === 'grouped' ? 'is-active' : '',
        text: t('view.grouped'),
        onclick: setView('grouped'),
      }),
      h('button', {
        class: gfState.view === 'flat' ? 'is-active' : '',
        text: t('view.flat'),
        onclick: setView('flat'),
      }),
    ),
  );
}

/* Every file of every game, which is literally what this screen is named after. */
function gfFlatTable(games) {
  const rows = [];
  for (const g of games) {
    for (const f of g.files) {
      rows.push(
        h(
          'tr',
          {},
          h('td', {}, h('span', { class: 'path', text: f.name })),
          h('td', { text: g.display_title }),
          h('td', {}, h('span', { class: 'chip chip-role', text: t(`role.${f.role}`) })),
          h(
            'td',
            {},
            g.title_id ? h('span', { class: 'chip chip-titleid', text: g.title_id }) : null,
          ),
          h('td', { text: f.app_ver || '' }),
          h('td', { class: 'num', text: fmtBytes(f.size_bytes) }),
        ),
      );
    }
  }

  return h(
    'table',
    { class: 'table' },
    h(
      'thead',
      {},
      h(
        'tr',
        {},
        h('th', { text: t('label.file') }),
        h('th', { text: t('label.game') }),
        h('th', { text: t('label.role') }),
        h('th', { text: t('label.titleId') }),
        h('th', { text: t('label.appVer') }),
        h('th', { class: 'num', text: t('label.size') }),
      ),
    ),
    h('tbody', {}, rows),
  );
}

function gfDamaged(damaged) {
  return h(
    'div',
    { class: 'review-card', style: 'margin-block-start:16px' },
    h('h2', { class: 'review-title', text: `${t('section.damaged')} (${fmtNum(damaged.length)})` }),
    h('div', { class: 'notice notice-warn', text: t('msg.damagedNote') }),
    h(
      'div',
      { class: 'file-list' },
      damaged.map((f) =>
        h(
          'div',
          { class: 'file-row' },
          h('span', { class: 'path', title: `${f.rel_dir}\\${f.name}`, text: f.name }),
          h('span', { text: f.detail }),
          h('span', { text: fmtBytes(f.size_bytes) }),
        ),
      ),
    ),
  );
}

function gfProgressBox() {
  const p = gfState.progress;
  const box = h('div', { class: 'notice notice-info', id: 'gf-progress' });
  box.append(
    h('span', { class: 'spinner' }),
    p
      ? `${t('msg.reading')} — ${fmtNum(p.dirs_read)} ${t('label.folders')} · ` +
        `${fmtNum(p.games)} ${t('label.games')}${p.current ? ` · ${p.current}` : ''}`
      : t('msg.reading'),
  );
  return box;
}

/* Replaces just the progress line, so a long read does not rebuild the whole screen on
 * every folder. */
function gfUpdateProgress() {
  const existing = document.getElementById('gf-progress');
  if (existing) existing.replaceWith(gfProgressBox());
}

function gfStatsLine(stats) {
  const parts = [
    `${fmtNum(stats.dirs_read)} ${t('label.folders')}`,
    `${fmtNum(stats.probed)} ${t('label.probed')}`,
    `${fmtNum(stats.from_cache)} ${t('label.fromCache')}`,
    `${fmtNum(stats.covers_embedded)} ${t('label.coversFromFiles')}`,
    `${stats.elapsed_ms} ms`,
  ];
  return h('div', { class: 'job-stats' }, parts.map((s) => h('span', { text: s })));
}

/* ------------------------------------------------------------------- render */

async function renderGameFiles() {
  // The folder is read when the window opens, which is the whole point of this screen.
  // `gfRead` repaints when it finishes, so this hands off rather than falling through.
  if (gfState.root && !gfState.data && !gfState.reading && !gfState.error) {
    await gfRead(gfState.root);
    return;
  }

  clear(el.view);
  el.view.append(gfToolbar());

  if (gfState.error) {
    el.view.append(h('div', { class: 'notice notice-problem', text: gfState.error }));
    return;
  }
  if (gfState.reading) {
    el.view.append(gfProgressBox());
    return;
  }
  if (!gfState.data) {
    el.view.append(emptyState('empty.gamefiles', 'empty.gamefilesSub'));
    return;
  }

  const { games, damaged, stats } = gfState.data;
  const search = (state.search || '').toLowerCase();
  const shown = games.filter((g) => {
    if (gfState.platform && g.platform !== gfState.platform) return false;
    if (!search) return true;
    // A disc image has no title id, so searching must not assume one.
    return (
      g.display_title.toLowerCase().includes(search) ||
      (g.title_id || '').toLowerCase().includes(search)
    );
  });

  if (games.length === 0) {
    el.view.append(emptyState('empty.noConsoleGames', 'empty.noConsoleGamesSub'));
    if (damaged.length) el.view.append(gfDamaged(damaged));
    return;
  }

  const total = shown.reduce((sum, g) => sum + Number(g.total_bytes), 0);
  el.view.append(
    gfFilters(games),
    h(
      'div',
      { class: 'section-head' },
      h('h2', { text: `${fmtNum(shown.length)} ${t('label.games')}` }),
      h('span', { class: 'count-pill', text: fmtBytes(total) }),
    ),
    gfStatsLine(stats),
  );

  if (shown.length === 0) {
    el.view.append(emptyState('empty.noMatch', null));
  } else if (gfState.view === 'flat') {
    el.view.append(gfFlatTable(shown));
  } else {
    el.view.append(h('div', { class: 'grid grid-square' }, shown.map(consoleCard)));
  }

  if (damaged.length) el.view.append(gfDamaged(damaged));
}
