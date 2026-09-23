/* Settings: metadata credentials and library enrichment.
 *
 * Artwork on this network takes an indirect route. IGDB's API is reachable but its
 * image CDN is not, and Steam's store API has been withdrawn while Steam's image CDN
 * still works. So IGDB supplies the match and the Steam app id, and the cover comes
 * from Steam's CDN. That needs free Twitch credentials, which is what this screen
 * collects — and the app is fully usable without them.
 */

const enrichState = { running: false, done: 0, total: 0, current: '', matched: 0, covers: 0, lastError: null };

async function renderSettings() {
  clear(el.view);

  let status;
  try {
    status = await invoke('credential_status');
  } catch (e) {
    el.view.append(h('div', { class: 'notice notice-problem', text: String(e?.message || e) }));
    return;
  }

  // One field: RAWG authenticates with a single key on the query string. The stored key
  // is never sent back to the page, so the placeholder shows only its last four
  // characters — enough to recognise which key is saved.
  const keyInput = h('input', {
    type: 'text',
    dir: 'ltr',
    placeholder: status.has_credentials ? `${status.key_hint}  ${t('msg.keyStored')}` : 'RAWG API key',
  });
  const result = h('div', {});

  const saveBtn = h('button', {
    class: 'btn btn-primary',
    text: t('action.saveAndTest'),
    onclick: async () => {
      saveBtn.disabled = true;
      result.replaceChildren(h('div', { class: 'notice notice-info' }, h('span', { class: 'spinner' }), t('msg.checking')));
      try {
        await invoke('save_credentials', { args: { api_key: keyInput.value } });
        result.replaceChildren(h('div', { class: 'notice notice-info', text: t('msg.credsOk') }));
        keyInput.value = '';
        renderSettings();
      } catch (e) {
        result.replaceChildren(h('div', { class: 'notice notice-problem', text: String(e?.message || e) }));
      } finally {
        saveBtn.disabled = false;
      }
    },
  });
  keyInput.addEventListener('keydown', (e) => {
    if (e.key === 'Enter') saveBtn.click();
  });

  const enrichBtn = h('button', {
    class: 'btn btn-primary',
    text: t('action.fetchCovers'),
    disabled: !status.has_credentials || enrichState.running || status.pending_count === 0,
    onclick: async () => {
      enrichBtn.disabled = true;
      enrichState.running = true;
      enrichState.lastError = null;
      try {
        const n = await invoke('enrich_library', { limit: 200 });
        if (n === 0) {
          enrichState.running = false;
          setStatus(t('msg.nothingToFetch'));
        }
      } catch (e) {
        enrichState.running = false;
        setStatus(String(e?.message || e), true);
      }
      renderSettings();
    },
  });

  const progressBox = h('div', {});
  if (enrichState.running || enrichState.done) {
    const pct = enrichState.total ? (enrichState.done / enrichState.total) * 100 : 0;
    progressBox.append(
      h('div', { class: 'progress', style: 'margin-block-start:12px' },
        h('div', { class: 'progress-fill', style: `inline-size:${pct}%` })),
      h('div', { class: 'job-stats' },
        h('span', { text: `${enrichState.done} / ${enrichState.total}` }),
        h('span', { text: `${t('label.matched')}: ${enrichState.matched}` }),
        h('span', { text: `${t('label.covers')}: ${enrichState.covers}` }),
        enrichState.current ? h('span', { text: enrichState.current }) : null),
      enrichState.lastError
        ? h('div', { class: 'notice notice-warn', style: 'margin-block-start:8px', text: enrichState.lastError })
        : null,
    );
  }

  el.view.append(
    h(
      'div',
      { class: 'review-card' },
      h('h2', { class: 'review-title', text: t('title.metadata') }),
      h('div', { class: 'notice notice-info', text: t('msg.metadataExplain') }),
      h('ol', { class: 'steps' },
        h('li', {}, t('msg.step1'), ' ', h('code', { class: 'path', text: 'rawg.io/apidocs' })),
        h('li', { text: t('msg.step2') }),
        h('li', { text: t('msg.step3') })),
      h('div', { class: 'field' },
        h('span', { class: 'field-label', text: t('label.apiKey') }), keyInput,
        h('div', { class: 'switch-note', text: t('msg.secretNote') })),
      h('div', { class: 'review-actions' },
        saveBtn,
        status.has_credentials
          ? h('button', {
              class: 'btn btn-ghost',
              text: t('action.removeCreds'),
              onclick: async () => { await invoke('clear_credentials'); renderSettings(); },
            })
          : null),
      result,
    ),
    h(
      'div',
      { class: 'review-card', style: 'margin-block-start:16px' },
      h('h2', { class: 'review-title', text: t('title.covers') }),
      h('div', { class: 'job-stats' },
        h('span', { text: `${t('label.pending')}: ${status.pending_count}` })),
      h('div', { class: 'review-actions' }, enrichBtn),
      progressBox,
      h('div', { class: 'switch-note', style: 'margin-block-start:12px' },
        `${t('label.coverFolder')}: `),
      h('div', { class: 'path', text: status.cover_dir }),
    ),
    resetCard(),
  );
}

/* Clearing everything the app has stored.
 *
 * Kept visually apart and behind a confirmation because it cannot be undone. The copy
 * states what is removed and, just as importantly, what is not: the catalogue describes
 * the drives, and deleting a description cannot delete what it describes. */
function resetCard() {
  const result = h('div', {});

  const doReset = async () => {
    try {
      const s = await invoke('reset_app_data', { confirm: 'RESET-EVERYTHING' });
      // The window's own remembered state goes too, or the next render would reopen a
      // folder the catalogue no longer knows about.
      try {
        localStorage.removeItem('gv.gamefiles.root');
      } catch {
        /* Blocked storage: nothing was remembered anyway. */
      }
      if (typeof gfState === 'object') {
        gfState.root = null;
        gfState.data = null;
        gfState.platform = null;
      }
      state.items = [];
      await loadDrives();
      await updateReviewBadge();
      setStatus(
        `${t('msg.resetDone')} — ${fmtNum(s.covers_removed)} ${t('label.covers')}, ${fmtBytes(s.bytes_freed)}`,
      );
      renderSettings();
    } catch (e) {
      result.replaceChildren(h('div', { class: 'notice notice-problem', text: String(e?.message || e) }));
    }
  };

  const confirmBtn = h('button', {
    class: 'btn btn-danger',
    text: t('action.resetConfirm'),
    onclick: () => {
      const { close } = modal(
        t('title.reset'),
        h(
          'div',
          {},
          h('div', { class: 'notice notice-problem', text: t('msg.resetWarn') }),
          h('ul', { class: 'steps' },
            h('li', { text: t('msg.resetItem1') }),
            h('li', { text: t('msg.resetItem2') }),
            h('li', { text: t('msg.resetItem3') })),
          h('div', { class: 'notice notice-info', text: t('msg.resetSafe') }),
        ),
        [
          h('button', { class: 'btn', text: t('action.cancel'), onclick: () => close() }),
          h('button', {
            class: 'btn btn-danger',
            text: t('action.resetConfirm'),
            onclick: () => {
              close();
              doReset();
            },
          }),
        ],
      );
    },
  });

  return h(
    'div',
    { class: 'review-card is-danger', style: 'margin-block-start:16px' },
    h('h2', { class: 'review-title', text: t('title.reset') }),
    h('div', { class: 'switch-note', text: t('msg.resetExplain') }),
    h('div', { class: 'review-actions', style: 'margin-block-start:12px' }, confirmBtn),
    result,
  );
}

/* Enrichment runs on a background thread; these events keep the screen in step. */
function initMetaEvents() {
  window.__TAURI__.event.listen('meta://progress', ({ payload }) => {
    enrichState.done = payload.done;
    enrichState.total = payload.total;
    enrichState.current = payload.current;
    enrichState.matched = payload.matched;
    enrichState.covers = payload.covers;
    if (payload.error) enrichState.lastError = payload.error;
    if (payload.finished) {
      enrichState.running = false;
      enrichState.current = '';
      // Covers have landed; the library grid needs to pick them up.
      if (state.route === 'library') render();
    }
    if (state.route === 'settings') renderSettings();
  });
}
