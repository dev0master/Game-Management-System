/* Item detail, the transfer dialog, and the transfer queue.
 *
 * The transfer flow is deliberately two-step: choosing a destination shows a preflight
 * report, and only then does a confirm button appear. Problems that would otherwise
 * surface several gigabytes into a copy — no space, a file too large for the target
 * filesystem, a name collision — are stated before anything moves.
 */

/* Live and finished jobs, newest first. Held in memory: a transfer belongs to the
 * session that started it, and the catalogue is updated when it completes. */
const jobs = new Map();

function modal(titleText, bodyNode, footNodes) {
  const overlay = h('div', { class: 'overlay' });
  const close = () => overlay.remove();

  overlay.addEventListener('click', (e) => {
    if (e.target === overlay) close();
  });
  document.addEventListener('keydown', function esc(e) {
    if (e.key === 'Escape') {
      close();
      document.removeEventListener('keydown', esc);
    }
  });

  overlay.append(
    h(
      'div',
      { class: 'modal' },
      h(
        'div',
        { class: 'modal-head' },
        h('h2', { class: 'modal-title', text: titleText }),
        h('button', { class: 'icon-btn', text: '✕', title: 'Esc', onclick: close }),
      ),
      h('div', { class: 'modal-body' }, bodyNode),
      footNodes ? h('div', { class: 'modal-foot' }, footNodes) : null,
    ),
  );
  document.body.append(overlay);
  return { overlay, close };
}

/* ----------------------------------------------------------- item detail */

async function openDetail(itemId) {
  let detail;
  try {
    detail = await invoke('get_item', { itemId });
  } catch (e) {
    setStatus(e?.message || String(e), true);
    return;
  }
  if (!detail) return;

  const it = detail.item;
  const online = Boolean(detail.drive_mount);

  const facts = h(
    'dl',
    { class: 'kv' },
    h('dt', { text: t('nav.storage') }),
    h('dd', { text: fmtBytes(it.total_bytes) }),
    h('dt', { text: t('nav.drives') }),
    h('dd', {}, h('span', {
      class: `chip ${online ? 'chip-drive' : 'chip-offline'}`,
      text: online
        ? [it.drive_label, detail.drive_mount].filter((v, i, a) => v && a.indexOf(v) === i).join(' · ')
        : `${it.drive_label} · ${t('label.offline')}`,
    })),
    h('dt', { text: 'Type' }),
    h('dd', { text: t(`kind.${it.kind}`) }),
    it.repacker ? h('dt', { text: 'Repacker' }) : null,
    it.repacker ? h('dd', { text: it.repacker }) : null,
    it.release_group ? h('dt', { text: 'Group' }) : null,
    it.release_group ? h('dd', { text: it.release_group }) : null,
    it.year ? h('dt', { text: 'Year' }) : null,
    it.year ? h('dd', { text: String(it.year) }) : null,
    it.edition ? h('dt', { text: 'Edition' }) : null,
    it.edition ? h('dd', { text: it.edition }) : null,
    it.notes ? h('dt', { text: 'Notes' }) : null,
    it.notes ? h('dd', { text: it.notes }) : null,
  );

  const body = h(
    'div',
    {},
    facts,
    h('div', { class: 'field', style: 'margin-block-start:16px' },
      h('span', { class: 'field-label', text: 'Path' }),
      h('div', { class: 'path', text: it.rel_path })),
    it.set_complete === false
      ? h('div', { class: 'notice notice-warn', text: it.incomplete_reason || t('msg.incomplete') })
      : null,
    detail.files.length
      ? h(
          'div',
          { class: 'field' },
          h('span', { class: 'field-label', text: `${t('label.volumes')} (${detail.files.length})` }),
          h(
            'div',
            { class: 'file-list' },
            detail.files.map((f) =>
              h(
                'div',
                { class: 'file-row' },
                h('span', { class: 'part-no', text: f.part_index ?? '•' }),
                h('span', { class: 'path', style: 'flex:1', title: f.rel_path, text: f.rel_path }),
                h('span', { class: 'num', text: fmtBytes(f.size_bytes) }),
              ),
            ),
          ),
        )
      : null,
    !online
      ? h('div', { class: 'notice notice-info', text: t('msg.offlineNote') })
      : null,
  );

  const { close } = modal(
    it.display_title,
    body,
    [
      h('button', {
        class: 'btn btn-primary',
        text: t('action.copyTo'),
        disabled: !online,
        title: online ? '' : t('msg.needsConnected'),
        onclick: () => { close(); openTransfer(detail, 'copy'); },
      }),
      h('button', {
        class: 'btn',
        text: t('action.moveTo'),
        disabled: !online,
        title: online ? '' : t('msg.needsConnected'),
        onclick: () => { close(); openTransfer(detail, 'move'); },
      }),
      h('button', {
        class: 'btn btn-ghost',
        text: t('action.reveal'),
        disabled: !online,
        onclick: () => invoke('reveal_item', { itemId }).catch((e) => setStatus(String(e?.message || e), true)),
      }),
      h('span', { class: 'spacer' }),
      h('button', {
        class: 'btn btn-ghost',
        text: t('action.exclude'),
        onclick: async () => { await invoke('set_verdict', { itemId, verdict: 'not_game' }); close(); render(); },
      }),
    ],
  );
}

/* --------------------------------------------------------- transfer setup */

async function openTransfer(detail, kind) {
  const it = detail.item;
  await loadDrives();
  const targets = state.drives.filter(
    (d) => d.is_online && d.current_mount && d.volume_guid !== detail.drive_guid,
  );

  if (targets.length === 0) {
    modal(t('title.transfer'), h('div', { class: 'notice notice-warn', text: t('msg.noTargets') }), null);
    return;
  }

  const form = {
    guid: targets[0].volume_guid,
    subfolder: 'Games',
    kind,
    verify: 'size',
    collision: 'rename',
    // Safe mode starts on for a move. The user has to consciously allow deletion.
    safeMode: kind === 'move',
  };

  const report = h('div', {});
  const confirmBtn = h('button', {
    class: 'btn btn-primary',
    text: t('action.start'),
    disabled: true,
  });

  async function refreshPreflight() {
    report.replaceChildren(h('div', { class: 'notice notice-info' }, h('span', { class: 'spinner' }), t('msg.checking')));
    confirmBtn.disabled = true;
    let p;
    try {
      p = await invoke('preflight_transfer', {
        args: {
          item_id: it.id,
          dst_drive_guid: form.guid,
          dst_subfolder: form.subfolder,
          kind: form.kind,
          verify: form.verify,
          collision: form.collision,
          safe_mode: form.safeMode,
        },
      });
    } catch (e) {
      report.replaceChildren(h('div', { class: 'notice notice-problem', text: e?.message || String(e) }));
      return;
    }

    const mins = (s) => (s < 60 ? `${s}s` : `${Math.round(s / 60)} min`);
    const summary = h(
      'dl',
      { class: 'kv' },
      h('dt', { text: t('label.items') }),
      h('dd', { text: `${p.file_count} · ${fmtBytes(p.total_bytes)}` }),
      h('dt', { text: t('label.free') }),
      h('dd', { text: fmtBytes(p.dst_free_bytes) }),
      h('dt', { text: t('label.estimate') }),
      h('dd', {
        text: p.same_volume && form.kind === 'move'
          ? t('label.instant')
          : mins(p.estimated_secs),
      }),
      form.verify === 'hash'
        ? h('dt', { text: t('label.verifyCost') })
        : null,
      form.verify === 'hash'
        ? h('dd', { text: `+${mins(p.hash_verify_extra_secs)}` })
        : null,
    );

    report.replaceChildren(
      summary,
      h('div', { style: 'margin-block-start:12px' },
        p.problems.map((x) => h('div', { class: 'notice notice-problem', text: x.message })),
        p.warnings.map((x) => h('div', { class: 'notice notice-warn', text: x.message })),
        p.ok && p.problems.length === 0 && p.warnings.length === 0
          ? h('div', { class: 'notice notice-info', text: t('msg.readyToGo') })
          : null),
    );
    confirmBtn.disabled = !p.ok;
  }

  function segment(options, current, onPick, dangerValue) {
    const wrap = h('div', { class: 'seg' });
    for (const [value, label] of options) {
      wrap.append(
        h('button', {
          type: 'button',
          class: `${value === current() ? 'is-active' : ''}${value === dangerValue ? ' is-danger' : ''}`,
          text: label,
          onclick: () => {
            onPick(value);
            [...wrap.children].forEach((b) => b.classList.toggle('is-active', b.textContent === label));
            refreshPreflight();
          },
        }),
      );
    }
    return wrap;
  }

  const safeToggle = h('input', { type: 'checkbox', checked: form.safeMode });
  safeToggle.addEventListener('change', () => {
    form.safeMode = safeToggle.checked;
    refreshPreflight();
  });

  const subfolderInput = h('input', { type: 'text', value: form.subfolder });
  let debounce = null;
  subfolderInput.addEventListener('input', () => {
    form.subfolder = subfolderInput.value.trim() || 'Games';
    clearTimeout(debounce);
    debounce = setTimeout(refreshPreflight, 250);
  });

  const driveSelect = h('select', {});
  for (const d of targets) {
    driveSelect.append(
      h('option', { value: d.volume_guid },
        `${d.label || d.current_mount} · ${fmtBytes(d.free_bytes)} ${t('label.free')}`),
    );
  }
  driveSelect.addEventListener('change', () => {
    form.guid = driveSelect.value;
    refreshPreflight();
  });

  const body = h(
    'div',
    {},
    h('div', { style: 'margin-block-end:18px' }, report),
    h('div', { class: 'field' },
      h('span', { class: 'field-label', text: t('label.destination') }),
      driveSelect),
    h('div', { class: 'field' },
      h('span', { class: 'field-label', text: t('label.subfolder') }),
      subfolderInput),
    h('div', { class: 'field' },
      h('span', { class: 'field-label', text: t('label.operation') }),
      segment(
        [['copy', t('action.copy')], ['move', t('action.move')]],
        () => form.kind,
        (v) => { form.kind = v; form.safeMode = v === 'move' ? safeToggle.checked : form.safeMode; },
        'move',
      )),
    h('div', { class: 'field' },
      h('span', { class: 'field-label', text: t('label.verification') }),
      segment(
        [['size', t('verify.size')], ['size_mtime', t('verify.sizeTime')], ['hash', t('verify.hash')]],
        () => form.verify,
        (v) => { form.verify = v; },
      ),
      h('div', { class: 'switch-note', text: t('msg.hashNote') })),
    h('div', { class: 'field' },
      h('span', { class: 'field-label', text: t('label.onCollision') }),
      segment(
        [['rename', t('collide.rename')], ['skip', t('collide.skip')], ['overwrite', t('collide.overwrite')]],
        () => form.collision,
        (v) => { form.collision = v; },
        'overwrite',
      )),
    h('div', { class: 'field' },
      h('label', { class: 'switch' }, safeToggle, h('span', { text: t('label.safeMode') })),
      h('div', { class: 'switch-note', text: t('msg.safeModeNote') })),
  );

  const { close } = modal(`${t('title.transfer')} — ${it.display_title}`, body, [
    confirmBtn,
    h('span', { class: 'spacer' }),
  ]);

  confirmBtn.addEventListener('click', async () => {
    confirmBtn.disabled = true;
    try {
      const started = await invoke('start_transfer', {
        args: {
          item_id: it.id,
          dst_drive_guid: form.guid,
          dst_subfolder: form.subfolder,
          kind: form.kind,
          verify: form.verify,
          collision: form.collision,
          safe_mode: form.safeMode,
        },
      });
      jobs.set(started.job_id, {
        id: started.job_id,
        title: it.display_title,
        kind: form.kind,
        safeMode: form.safeMode,
        filesTotal: started.files,
        bytesTotal: started.total_bytes,
        bytesDone: 0,
        filesDone: 0,
        rate: 0,
        eta: 0,
        phase: 'copying',
        outcome: null,
      });
      close();
      go('transfers');
    } catch (e) {
      setStatus(e?.message || String(e), true);
      confirmBtn.disabled = false;
    }
  });

  refreshPreflight();
}

/* -------------------------------------------------------- transfer queue */

function renderTransfers() {
  clear(el.view);
  if (jobs.size === 0) {
    el.view.append(emptyState('empty.transfers', 'empty.transfersSub'));
    return;
  }

  for (const job of [...jobs.values()].reverse()) {
    const pct = job.bytesTotal ? Math.min(100, (job.bytesDone / job.bytesTotal) * 100) : 0;
    const done = job.phase === 'done' || job.phase === 'cancelled';
    const failed = job.outcome && !job.outcome.all_verified;

    const stats = [
      `${fmtBytes(job.bytesDone)} / ${fmtBytes(job.bytesTotal)}`,
      !done && job.rate ? `${fmtBytes(job.rate)}/s` : null,
      !done && job.eta ? `${t('label.eta')} ${job.eta < 60 ? `${job.eta}s` : `${Math.round(job.eta / 60)} min`}` : null,
      done && job.outcome ? `${job.outcome.elapsed_secs.toFixed(1)}s` : null,
    ].filter(Boolean);

    el.view.append(
      h(
        'div',
        { class: `job${done && !failed ? ' is-done' : ''}${failed ? ' is-failed' : ''}` },
        h(
          'div',
          { class: 'job-head' },
          h('span', { class: 'job-title', text: job.title }),
          h('span', { class: `chip ${job.kind === 'move' ? 'chip-warn' : ''}`,
            text: job.kind === 'move' ? t('action.move') : t('action.copy') }),
          job.safeMode && job.kind === 'move'
            ? h('span', { class: 'chip', text: t('label.safeMode') })
            : null,
          h('span', { class: 'count-pill', text: `${job.filesDone}/${job.filesTotal}` }),
          h(
            'div',
            { class: 'job-actions' },
            !done
              ? h('button', {
                  class: 'btn btn-sm',
                  text: t('action.cancel'),
                  onclick: async () => { await invoke('cancel_transfer', { jobId: job.id }); },
                })
              : null,
          ),
        ),
        h('div', { class: 'progress' },
          h('div', {
            class: `progress-fill${done && !failed ? ' is-done' : ''}${failed ? ' is-failed' : ''}`,
            style: `inline-size:${done && !failed ? 100 : pct}%`,
          })),
        h('div', { class: 'job-stats' }, stats.map((s) => h('span', { text: s }))),
        job.outcome?.delete_blocked_reason
          ? h('div', { class: 'notice notice-warn', style: 'margin-block-start:10px',
              text: job.outcome.delete_blocked_reason })
          : null,
        job.outcome && job.outcome.all_verified && !job.outcome.cancelled
          ? h('div', { class: 'notice notice-info', style: 'margin-block-start:10px',
              text: job.outcome.sources_deleted.length
                ? `${t('msg.moveDone')} — ${job.outcome.sources_deleted.length} ${t('msg.sourcesRemoved')}`
                : t('msg.copyDone') })
          : null,
        job.outcome?.files?.filter((f) => f.error).length
          ? h('div', { class: 'file-list', style: 'margin-block-start:10px' },
              job.outcome.files.filter((f) => f.error).map((f) =>
                h('div', { class: 'notice notice-problem', text: `${shortPath(f.src)}: ${f.error}` })))
          : null,
      ),
    );
  }
}

/* Wire the backend's progress and completion events into the queue view. */
function initTransferEvents() {
  const listen = window.__TAURI__.event.listen;

  listen('transfer://progress', ({ payload }) => {
    const job = jobs.get(payload.job_id);
    if (!job) return;
    job.bytesDone = payload.bytes_done;
    job.filesDone = payload.files_done;
    job.rate = payload.bytes_per_sec;
    job.eta = payload.eta_secs;
    job.phase = payload.phase;
    if (state.route === 'transfers') renderTransfers();
  });

  listen('transfer://finished', async ({ payload }) => {
    const job = jobs.get(payload.job_id);
    if (!job) return;
    job.outcome = payload.outcome;
    job.phase = payload.outcome.cancelled ? 'cancelled' : 'done';
    job.bytesDone = payload.outcome.bytes_copied;
    job.filesDone = job.filesTotal;
    if (state.route === 'transfers') renderTransfers();
    else if (state.route === 'library') render();
    await updateTransferBadge();
  });
}

async function updateTransferBadge() {
  const active = [...jobs.values()].filter((j) => j.phase !== 'done' && j.phase !== 'cancelled').length;
  const badge = document.getElementById('transfer-count');
  if (!badge) return;
  badge.hidden = active === 0;
  badge.textContent = String(active);
}
