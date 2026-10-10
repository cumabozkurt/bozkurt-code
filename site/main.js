(function () {
  var root = document.documentElement;

  // Language toggle (EN / TR)
  function setLang(lang) {
    root.setAttribute('data-lang', lang);
    root.setAttribute('lang', lang);
    try { localStorage.setItem('bz-lang', lang); } catch (e) {}
    document.querySelectorAll('[data-set-lang]').forEach(function (b) {
      b.setAttribute('aria-pressed', String(b.getAttribute('data-set-lang') === lang));
    });
    var sel = document.querySelector('.tabs [aria-selected="true"]');
    if (sel) caption(sel);
    document.querySelectorAll('[data-copy]').forEach(function (b) { b.textContent = lang === 'tr' ? 'Kopyala' : 'Copy'; });
  }
  document.querySelectorAll('[data-set-lang]').forEach(function (b) {
    b.addEventListener('click', function () { setLang(b.getAttribute('data-set-lang')); });
  });

  // Screenshot tabs
  var img = document.getElementById('shot');
  var cap = document.getElementById('shot-cap');
  function caption(btn) {
    cap.textContent = btn.getAttribute('data-cap-' + (root.getAttribute('data-lang') || 'en'));
  }
  document.querySelectorAll('.tabs [data-shot]').forEach(function (btn) {
    btn.addEventListener('click', function () {
      document.querySelectorAll('.tabs [data-shot]').forEach(function (b) { b.setAttribute('aria-selected', 'false'); });
      btn.setAttribute('aria-selected', 'true');
      img.src = 'screenshots/' + btn.getAttribute('data-shot') + '.png';
      img.alt = btn.textContent.trim();
      caption(btn);
    });
  });

  // Copy buttons
  document.querySelectorAll('[data-copy]').forEach(function (b) {
    b.addEventListener('click', function () {
      var text = b.parentElement.querySelector('code').textContent;
      var done = function () {
        var old = b.textContent;
        b.textContent = root.getAttribute('data-lang') === 'tr' ? 'Kopyalandı' : 'Copied';
        setTimeout(function () { b.textContent = old; }, 1400);
      };
      if (navigator.clipboard) navigator.clipboard.writeText(text).then(done, function () {});
    });
  });

  // Mobile menu
  var menu = document.querySelector('.menu-btn');
  var links = document.getElementById('nav-links');
  menu.addEventListener('click', function () {
    var open = links.classList.toggle('open');
    menu.setAttribute('aria-expanded', String(open));
  });
  links.addEventListener('click', function (e) {
    if (e.target.closest('a')) { links.classList.remove('open'); menu.setAttribute('aria-expanded', 'false'); }
  });

  setLang(root.getAttribute('data-lang') || 'en');
})();
