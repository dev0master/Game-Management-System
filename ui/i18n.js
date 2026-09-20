/* Translations and locale-aware formatting.
 *
 * Arabic is the default because the user's drives and folder names are Arabic; English
 * is one click away. Switching sets both `lang` and `dir` on <html>, which is all the
 * stylesheet needs to mirror the layout.
 */

const STRINGS = {
  ar: {
    'nav.settings': 'الإعدادات',
    'title.settings': 'الإعدادات',
    'title.metadata': 'مصدر معلومات الألعاب',
    'title.covers': 'جلب الأغلفة',
    'action.saveAndTest': 'حفظ واختبار',
    'action.removeCreds': 'حذف المفاتيح',
    'action.fetchCovers': 'جلب الأغلفة والمعلومات',
    'label.matched': 'تطابق',
    'label.covers': 'أغلفة',
    'label.pending': 'بانتظار الجلب',
    'label.coverFolder': 'مجلد الأغلفة',
    'msg.metadataExplain': 'شبكتك تحجب مواقع الأغلفة المعتادة، لكن IGDB يعمل ومخدّم صور Steam يعمل. لذلك نسأل IGDB عن اللعبة ونأخذ منه رقمها في Steam، ثم نجلب الغلاف من Steam. تحتاج مفاتيح Twitch مجانية:',
    'msg.step1': 'أنشئ تطبيقاً مجانياً على',
    'msg.step2': 'اختر OAuth Redirect URL = http://localhost و Category = Application Integration',
    'msg.step3': 'انسخ Client ID و Client Secret وضعهما هنا',
    'msg.secretNote': 'يُخزَّن مشفّراً بـ DPAPI على حسابك في ويندوز، ولا يُحفظ كنص صريح أبداً.',
    'msg.secretStored': 'محفوظ',
    'msg.credsOk': 'تم التحقق من المفاتيح وحفظها بنجاح.',
    'msg.nothingToFetch': 'كل الألعاب لديها أغلفة بالفعل.',
    'nav.transfers': 'النقل',
    'title.transfers': 'عمليات النقل',
    'title.transfer': 'نقل أو نسخ',
    'action.copyTo': 'نسخ إلى…',
    'action.moveTo': 'نقل إلى…',
    'action.copy': 'نسخ',
    'action.move': 'نقل',
    'action.reveal': 'فتح في المستكشف',
    'action.start': 'ابدأ',
    'action.cancel': 'إلغاء',
    'label.destination': 'الهارد الوجهة',
    'label.subfolder': 'المجلد داخل الهارد',
    'label.operation': 'العملية',
    'label.verification': 'التحقق من سلامة النسخ',
    'label.onCollision': 'إذا كان الملف موجوداً',
    'label.safeMode': 'الوضع الآمن — لا تحذف المصدر أبداً',
    'label.estimate': 'الوقت المتوقع',
    'label.verifyCost': 'زمن التحقق الإضافي',
    'label.instant': 'فوري (نفس الهارد)',
    'label.eta': 'المتبقي',
    'verify.size': 'الحجم',
    'verify.sizeTime': 'الحجم والتاريخ',
    'verify.hash': 'بصمة كاملة',
    'collide.rename': 'إعادة تسمية',
    'collide.skip': 'تخطٍّ',
    'collide.overwrite': 'استبدال',
    'msg.checking': 'جارٍ الفحص…',
    'msg.readyToGo': 'كل شيء جاهز، لا توجد أي مشاكل.',
    'msg.hashNote': 'البصمة الكاملة تكتشف أي تلف في النسخ، لكنها تضاعف الوقت تقريباً.',
    'msg.safeModeNote': 'عند تفعيله يتحول «النقل» إلى «نسخ» ولا يُحذف أي ملف من المصدر.',
    'msg.noTargets': 'لا يوجد هارد آخر موصول لتكون وجهة. وصّل هاردًا ثانياً ثم أعد المحاولة.',
    'msg.needsConnected': 'يجب أن يكون الهارد موصولاً',
    'msg.copyDone': 'اكتمل النسخ والتحقق بنجاح.',
    'msg.moveDone': 'اكتمل النقل والتحقق',
    'msg.sourcesRemoved': 'ملفات حُذفت من المصدر',
    'empty.transfers': 'لا توجد عمليات نقل',
    'empty.transfersSub': 'اختر لعبة من المكتبة ثم «نسخ إلى…» أو «نقل إلى…».',
    'nav.library': 'المكتبة',
    'nav.drives': 'الهاردات',
    'nav.review': 'بحاجة لمراجعة',
    'nav.filtered': 'المستبعدات',
    'nav.duplicates': 'التكرار',
    'nav.storage': 'المساحة',

    'title.library': 'المكتبة',
    'title.drives': 'الهاردات',
    'title.review': 'بحاجة لمراجعة',
    'title.filtered': 'العناصر المستبعدة',
    'title.duplicates': 'الألعاب المكررة',
    'title.storage': 'استهلاك المساحة',

    'action.scan': 'فحص هارد',
    'action.rescan': 'إعادة فحص',
    'action.scanning': 'جارٍ الفحص…',
    'action.isGame': 'لعبة',
    'action.notGame': 'ليست لعبة',
    'action.skip': 'تخطٍّ',
    'action.restore': 'إرجاع للمكتبة',
    'action.exclude': 'استبعاد',

    'search.placeholder': 'ابحث عن لعبة…',

    'label.offline': 'غير متصل',
    'label.online': 'متصل',
    'label.games': 'ألعاب',
    'label.items': 'عناصر',
    'label.free': 'متاح',
    'label.of': 'من',
    'label.volumes': 'أجزاء',
    'label.lastSeen': 'آخر ظهور',
    'label.lastScan': 'آخر فحص',
    'label.never': 'لم يُفحص بعد',
    'label.confidence': 'الثقة',
    'label.copies': 'نسخ',
    'label.reclaimable': 'يمكن توفير',
    'label.noJournal': 'بدون journaling — الفصل المفاجئ قد يتلف الملفات',
    'label.maxFile': 'أقصى حجم ملف',
    'label.catalogued': 'مفهرس',
    'label.other': 'أخرى',
    'label.freeSpace': 'مساحة فارغة',

    'empty.library': 'لا توجد ألعاب في الفهرس بعد',
    'empty.librarySub': 'اضغط «فحص هارد» لبدء فهرسة أول هارد.',
    'empty.review': 'لا شيء بحاجة لمراجعة',
    'empty.reviewSub': 'كل ما عُثر عليه صُنِّف بثقة كافية.',
    'empty.filtered': 'لم يُستبعد أي شيء',
    'empty.duplicates': 'لا توجد ألعاب مكررة',
    'empty.duplicatesSub': 'لا توجد لعبة مخزَّنة على أكثر من هارد.',
    'empty.drives': 'لم يُفهرس أي هارد بعد',

    'msg.scanDone': 'اكتمل الفحص',
    'msg.found': 'عُثر على',
    'msg.new': 'جديد',
    'msg.missing': 'مفقود',
    'msg.in': 'خلال',
    'msg.pickDrive': 'اختر هاردًا لفحصه:',
    'msg.offlineNote': 'هذا الهارد غير موصول الآن. المحتويات معروضة من الفهرس المحفوظ.',
    'msg.filteredNote': 'هذه العناصر لم تُصنَّف كألعاب. لا شيء يُحذف — يمكنك إرجاع أي عنصر للمكتبة.',
    'msg.incomplete': 'مجموعة ناقصة',

    'kind.archive_set': 'أرشيف مضغوط',
    'kind.installed_game': 'لعبة مفكوكة',
    'kind.iso': 'صورة قرص',
    'kind.media': 'وسائط',
    'kind.dev_project': 'مشروع برمجي',
    'kind.utility': 'أداة',
    'kind.unknown': 'غير معروف',
  },

  en: {
    'nav.settings': 'Settings',
    'title.settings': 'Settings',
    'title.metadata': 'Game metadata source',
    'title.covers': 'Fetch covers',
    'action.saveAndTest': 'Save and test',
    'action.removeCreds': 'Remove keys',
    'action.fetchCovers': 'Fetch covers and details',
    'label.matched': 'matched',
    'label.covers': 'covers',
    'label.pending': 'Pending',
    'label.coverFolder': 'Cover cache folder',
    'msg.metadataExplain': 'Your network blocks the usual artwork sources, but IGDB works and so does Steam’s image CDN. So IGDB supplies the match and the Steam app id, and the cover comes from Steam. This needs free Twitch credentials:',
    'msg.step1': 'Create a free application at',
    'msg.step2': 'Set OAuth Redirect URL to http://localhost and Category to Application Integration',
    'msg.step3': 'Copy the Client ID and Client Secret here',
    'msg.secretNote': 'Stored encrypted with Windows DPAPI against your account, never as plain text.',
    'msg.secretStored': 'stored',
    'msg.credsOk': 'Credentials verified and saved.',
    'msg.nothingToFetch': 'Every game already has artwork.',
    'nav.transfers': 'Transfers',
    'title.transfers': 'Transfers',
    'title.transfer': 'Copy or move',
    'action.copyTo': 'Copy to…',
    'action.moveTo': 'Move to…',
    'action.copy': 'Copy',
    'action.move': 'Move',
    'action.reveal': 'Show in Explorer',
    'action.start': 'Start',
    'action.cancel': 'Cancel',
    'label.destination': 'Destination drive',
    'label.subfolder': 'Folder on that drive',
    'label.operation': 'Operation',
    'label.verification': 'Verification',
    'label.onCollision': 'If the file already exists',
    'label.safeMode': 'Safe mode — never delete the source',
    'label.estimate': 'Estimated time',
    'label.verifyCost': 'Extra time to verify',
    'label.instant': 'Instant (same drive)',
    'label.eta': 'Remaining',
    'verify.size': 'Size',
    'verify.sizeTime': 'Size + date',
    'verify.hash': 'Full hash',
    'collide.rename': 'Rename',
    'collide.skip': 'Skip',
    'collide.overwrite': 'Replace',
    'msg.checking': 'Checking…',
    'msg.readyToGo': 'All checks passed.',
    'msg.hashNote': 'A full hash catches any corruption, but roughly doubles the time.',
    'msg.safeModeNote': 'With this on, a move becomes a copy and no source file is ever deleted.',
    'msg.noTargets': 'No other drive is connected to copy to. Connect a second drive and try again.',
    'msg.needsConnected': 'the drive must be connected',
    'msg.copyDone': 'Copied and verified successfully.',
    'msg.moveDone': 'Moved and verified',
    'msg.sourcesRemoved': 'source files removed',
    'empty.transfers': 'No transfers yet',
    'empty.transfersSub': 'Pick a game in the library, then “Copy to…” or “Move to…”.',
    'nav.library': 'Library',
    'nav.drives': 'Drives',
    'nav.review': 'Needs review',
    'nav.filtered': 'Filtered out',
    'nav.duplicates': 'Duplicates',
    'nav.storage': 'Storage',

    'title.library': 'Library',
    'title.drives': 'Drives',
    'title.review': 'Needs review',
    'title.filtered': 'Filtered out',
    'title.duplicates': 'Duplicate games',
    'title.storage': 'Storage usage',

    'action.scan': 'Scan a drive',
    'action.rescan': 'Rescan',
    'action.scanning': 'Scanning…',
    'action.isGame': 'It is a game',
    'action.notGame': 'Not a game',
    'action.skip': 'Skip',
    'action.restore': 'Add to library',
    'action.exclude': 'Exclude',

    'search.placeholder': 'Search games…',

    'label.offline': 'Offline',
    'label.online': 'Online',
    'label.games': 'games',
    'label.items': 'items',
    'label.free': 'free',
    'label.of': 'of',
    'label.volumes': 'volumes',
    'label.lastSeen': 'Last seen',
    'label.lastScan': 'Last scan',
    'label.never': 'never scanned',
    'label.confidence': 'Confidence',
    'label.copies': 'copies',
    'label.reclaimable': 'reclaimable',
    'label.noJournal': 'no journalling — an unsafe eject can corrupt files',
    'label.maxFile': 'max file size',
    'label.catalogued': 'Catalogued',
    'label.other': 'Other',
    'label.freeSpace': 'Free space',

    'empty.library': 'Nothing catalogued yet',
    'empty.librarySub': 'Choose “Scan a drive” to index your first drive.',
    'empty.review': 'Nothing to review',
    'empty.reviewSub': 'Everything found was classified confidently.',
    'empty.filtered': 'Nothing was filtered out',
    'empty.duplicates': 'No duplicates found',
    'empty.duplicatesSub': 'No game is stored on more than one drive.',
    'empty.drives': 'No drives catalogued yet',

    'msg.scanDone': 'Scan complete',
    'msg.found': 'found',
    'msg.new': 'new',
    'msg.missing': 'missing',
    'msg.in': 'in',
    'msg.pickDrive': 'Pick a drive to scan:',
    'msg.offlineNote': 'This drive is not connected. Contents are shown from the saved catalogue.',
    'msg.filteredNote': 'These were not classified as games. Nothing is deleted — you can add any of them back.',
    'msg.incomplete': 'Incomplete set',

    'kind.archive_set': 'Archive',
    'kind.installed_game': 'Installed game',
    'kind.iso': 'Disc image',
    'kind.media': 'Media',
    'kind.dev_project': 'Dev project',
    'kind.utility': 'Utility',
    'kind.unknown': 'Unknown',
  },
};

let lang = 'ar';

function t(key) {
  return STRINGS[lang][key] ?? STRINGS.en[key] ?? key;
}

function setLang(next) {
  lang = next;
  const html = document.documentElement;
  html.lang = next;
  html.dir = next === 'ar' ? 'rtl' : 'ltr';
  try {
    localStorage.setItem('gv.lang', next);
  } catch {
    /* Private mode or blocked storage: the choice just will not persist. */
  }
  applyStaticStrings();
}

function currentLang() {
  return lang;
}

function loadLang() {
  let saved = null;
  try {
    saved = localStorage.getItem('gv.lang');
  } catch {
    saved = null;
  }
  setLang(saved === 'en' || saved === 'ar' ? saved : 'ar');
}

function applyStaticStrings() {
  document.querySelectorAll('[data-i18n]').forEach((el) => {
    el.textContent = t(el.dataset.i18n);
  });
  document.querySelectorAll('[data-i18n-ph]').forEach((el) => {
    el.placeholder = t(el.dataset.i18nPh);
  });
  const label = document.getElementById('lang-label');
  if (label) label.textContent = lang === 'ar' ? 'EN' : 'ع';
}

/* Sizes always use Latin digits, even in Arabic: a column of Arabic-Indic numerals is
 * markedly harder to compare at a glance, and these are figures the user scans. */
function fmtBytes(bytes) {
  const n = Number(bytes) || 0;
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = n;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i += 1;
  }
  const digits = i === 0 ? 0 : v < 10 ? 1 : v < 100 ? 1 : 0;
  return `${v.toLocaleString('en-US', {
    minimumFractionDigits: digits,
    maximumFractionDigits: digits,
  })} ${units[i]}`;
}

function fmtNum(n) {
  return Number(n || 0).toLocaleString('en-US');
}

/* Relative time, from the ISO-8601 UTC strings the catalogue stores. */
function fmtWhen(iso) {
  if (!iso) return t('label.never');
  const then = Date.parse(iso.endsWith('Z') ? iso : `${iso}Z`);
  if (Number.isNaN(then)) return iso;
  const secs = Math.max(0, (Date.now() - then) / 1000);
  const rtf = new Intl.RelativeTimeFormat(lang === 'ar' ? 'ar' : 'en', { numeric: 'auto' });
  const steps = [
    [60, 'second', 1],
    [3600, 'minute', 60],
    [86400, 'hour', 3600],
    [2592000, 'day', 86400],
    [31536000, 'month', 2592000],
    [Infinity, 'year', 31536000],
  ];
  for (const [limit, unit, div] of steps) {
    if (secs < limit) return rtf.format(-Math.round(secs / div), unit);
  }
  return iso;
}
