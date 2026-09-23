/* Translations and locale-aware formatting.
 *
 * Arabic is the default because the user's drives and folder names are Arabic; English
 * is one click away. Switching sets both `lang` and `dir` on <html>, which is all the
 * stylesheet needs to mirror the layout.
 */

const STRINGS = {
  ar: {
    /* ---- ملفات الألعاب ---- */
    'nav.gamefiles': 'ملفات الألعاب',
    'title.gamefiles': 'ملفات الألعاب',
    'action.readFolder': 'اقرأ المجلد',
    'filter.allPlatforms': 'كل الأجهزة',
    'platform.ps4': 'PlayStation 4',
    'platform.xbox360': 'Xbox 360',
    'platform.xbox': 'Xbox الأصلي',
    'platform.ps1': 'PlayStation 1',
    'platform.ps2': 'PlayStation 2',
    'platform.ps3': 'PlayStation 3',
    'platform.ps5': 'PlayStation 5',
    'platform.psp': 'PSP',
    'platform.psvita': 'PS Vita',
    'platform.xboxseries': 'Xbox One / Series',
    'platform.unknown': 'جهاز غير محدّد',
    'label.guessed': 'ترجيح',
    'label.titleSource': 'مصدر الاسم',
    'title.fromFile': 'من داخل ملف اللعبة',
    'title.fromFilename': 'من اسم الملف',
    'msg.platformGuessed': 'الجهاز مُستنتَج من اسم المجلد، لا من داخل الملف',
    'view.grouped': 'مجمّعة',
    'view.flat': 'الملفات',
    'label.titleId': 'رقم اللعبة',
    'label.platform': 'الجهاز',
    'label.firmware': 'يحتاج',
    'label.folderPath': 'مسار المجلد، مثل E:\\Games',
    'label.folder': 'المجلد',
    'label.folders': 'مجلد',
    'label.probed': 'ملف مقروء',
    'label.fromCache': 'من الذاكرة',
    'label.coversFromFiles': 'غلاف من داخل الملفات',
    'label.coverFromFile': 'من الملف',
    'label.coverSource': 'مصدر الغلاف',
    'label.addonFor': 'إضافة لـ',
    'label.appVer': 'الإصدار',
    'label.role': 'النوع',
    'label.file': 'الملف',
    'label.game': 'اللعبة',
    'label.size': 'الحجم',
    'role.pkg_game': 'لعبة',
    'role.pkg_update': 'تحديث',
    'role.pkg_dlc': 'إضافة',
    'role.pkg_part': 'جزء',
    'role.cover': 'صورة',
    'role.data': 'بيانات',
    'cover.pkg_icon0': 'من داخل ملف اللعبة',
    'cover.pkg_sibling': 'صورة بجانب الملف',
    'cover.none': 'لا يوجد غلاف',
    'section.damaged': 'ملفات تالفة أو غير مكتملة',
    'msg.reading': 'جارٍ قراءة المجلد…',
    'msg.damagedNote': 'هذه الملفات لا تحتوي ترويسة صالحة ولا تنتمي إلى مجموعة مقسّمة. تُعرض كما هي ولا يُحذف منها شيء.',
    'empty.gamefiles': 'اختر مجلد الألعاب',
    'empty.gamefilesSub': 'اضغط على أحد الأقراص أعلاه، أو اكتب مسار المجلد الذي يحتوي ملفات ألعابك. تُقرأ المجلدات مباشرة في كل مرة تُفتح النافذة.',
    'empty.noConsoleGames': 'لا توجد ملفات ألعاب أجهزة في هذا المجلد',
    'empty.noConsoleGamesSub': 'لم نجد أي حزمة PS4 بامتداد .pkg هنا. جرّب مجلداً آخر أو القرص كاملاً.',
    'empty.noMatch': 'لا نتائج مطابقة',
    'kind.console_game': 'لعبة جهاز',

    'title.reset': 'تصفير البرنامج',
    'action.resetConfirm': 'احذف كل شيء محفوظ',
    'msg.resetExplain': 'يمسح كل ما خزّنه البرنامج ويعيده كأنه جديد. لا يُحذف أي ملف من الهاردات إطلاقاً.',
    'msg.resetWarn': 'هذا الإجراء لا يمكن التراجع عنه.',
    'msg.resetItem1': 'فهرس الألعاب كاملاً — كل ما فُحص وكل تصنيف عدّلته بنفسك',
    'msg.resetItem2': 'مجلد الأغلفة المحفوظة',
    'msg.resetItem3': 'مفتاح RAWG المخزّن',
    'msg.resetSafe': 'لا يُحذف شيء من الهاردات. الفهرس وصفٌ لمحتواها فقط، ومسح الوصف لا يمسح الموصوف. ألعابك وملفاتك تبقى كما هي.',
    'msg.resetDone': 'تم تصفير البرنامج',
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
    'msg.metadataExplain': 'مصدر المعلومات هو RAWG — مجاني ولا يحتاج Twitch. نسأله عن اللعبة، وإن كانت موجودة على Steam نجلب غلافها الطولي من مخدّم صور Steam، وإلا نأخذ صورة RAWG نفسها. يغطي RAWG ألعاب الأجهزة أيضاً لا ألعاب الحاسب فقط. تحتاج مفتاحاً مجانياً:',
    'msg.step1': 'أنشئ حساباً مجانياً واطلب مفتاحاً من',
    'msg.step2': 'المفتاح يظهر مباشرة في صفحة API key بعد التسجيل',
    'msg.step3': 'انسخ المفتاح والصقه هنا ثم اضغط حفظ واختبار',
    'msg.secretNote': 'يُخزَّن مشفّراً بـ DPAPI على حسابك في ويندوز، ولا يُحفظ كنص صريح أبداً.',
    'msg.secretStored': 'محفوظ',
    'msg.keyStored': 'محفوظ',
    'label.apiKey': 'مفتاح RAWG',
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
    /* ---- Game Files ---- */
    'nav.gamefiles': 'Game Files',
    'title.gamefiles': 'Game Files',
    'action.readFolder': 'Read folder',
    'filter.allPlatforms': 'All consoles',
    'platform.ps4': 'PlayStation 4',
    'platform.xbox360': 'Xbox 360',
    'platform.xbox': 'Original Xbox',
    'platform.ps1': 'PlayStation 1',
    'platform.ps2': 'PlayStation 2',
    'platform.ps3': 'PlayStation 3',
    'platform.ps5': 'PlayStation 5',
    'platform.psp': 'PSP',
    'platform.psvita': 'PS Vita',
    'platform.xboxseries': 'Xbox One / Series',
    'platform.unknown': 'Console not identified',
    'label.guessed': 'guess',
    'label.titleSource': 'Name from',
    'title.fromFile': 'inside the game file',
    'title.fromFilename': 'the filename',
    'msg.platformGuessed': 'the console is inferred from the folder name, not read from the file',
    'view.grouped': 'Grouped',
    'view.flat': 'Files',
    'label.titleId': 'Title ID',
    'label.platform': 'Console',
    'label.firmware': 'Needs',
    'label.folderPath': 'Folder path, e.g. E:\\Games',
    'label.folder': 'Folder',
    'label.folders': 'folders',
    'label.probed': 'read',
    'label.fromCache': 'cached',
    'label.coversFromFiles': 'covers from the files',
    'label.coverFromFile': 'from file',
    'label.coverSource': 'Cover source',
    'label.addonFor': 'Add-on for',
    'label.appVer': 'Version',
    'label.role': 'Type',
    'label.file': 'File',
    'label.game': 'Game',
    'label.size': 'Size',
    'role.pkg_game': 'game',
    'role.pkg_update': 'update',
    'role.pkg_dlc': 'DLC',
    'role.pkg_part': 'part',
    'role.cover': 'image',
    'role.data': 'data',
    'cover.pkg_icon0': 'read from inside the game file',
    'cover.pkg_sibling': 'image beside the file',
    'cover.none': 'no cover',
    'section.damaged': 'Damaged or incomplete files',
    'msg.reading': 'Reading the folder…',
    'msg.damagedNote': 'These files carry no valid header and belong to no split set. They are shown as they are; nothing is deleted.',
    'empty.gamefiles': 'Choose a game folder',
    'empty.gamefilesSub': 'Pick one of the drives above, or type the path of the folder holding your game files. The folder is read fresh every time this window opens.',
    'empty.noConsoleGames': 'No console game files in this folder',
    'empty.noConsoleGamesSub': 'No PS4 .pkg packages were found here. Try another folder, or the whole drive.',
    'empty.noMatch': 'Nothing matches',
    'kind.console_game': 'Console game',

    'title.reset': 'Reset the app',
    'action.resetConfirm': 'Delete everything stored',
    'msg.resetExplain': 'Erases everything the app has stored and starts over as if new. No file on any drive is deleted.',
    'msg.resetWarn': 'This cannot be undone.',
    'msg.resetItem1': 'the whole catalogue — every scan, and every verdict you set yourself',
    'msg.resetItem2': 'the saved cover images',
    'msg.resetItem3': 'the stored RAWG key',
    'msg.resetSafe': 'Nothing on any drive is touched. The catalogue only describes what is on them, and deleting a description cannot delete what it describes. Your games stay exactly where they are.',
    'msg.resetDone': 'The app has been reset',
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
    'msg.metadataExplain': 'Artwork comes from RAWG — free, and no Twitch account involved. RAWG supplies the match; when the game is on Steam the portrait box art is fetched from Steam’s image CDN, otherwise RAWG’s own artwork is used. RAWG covers console titles too, not just PC. This needs a free key:',
    'msg.step1': 'Create a free account and request a key at',
    'msg.step2': 'The key is shown on the API key page as soon as you register',
    'msg.step3': 'Paste the key here and press Save and test',
    'msg.secretNote': 'Stored encrypted with Windows DPAPI against your account, never as plain text.',
    'msg.secretStored': 'stored',
    'msg.keyStored': 'stored',
    'label.apiKey': 'RAWG API key',
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
