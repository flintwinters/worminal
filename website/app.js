(() => {
  const links = [...document.querySelectorAll('[data-section]')];
  const sections = [...document.querySelectorAll('[data-panel]')];
  const status = document.getElementById('status-message');
  const find = document.getElementById('find');
  const rows = [...document.querySelectorAll('#capabilities tbody tr')];
  const filters = [...document.querySelectorAll('[data-filter]')];
  let group = 'all';

  function markSection(view) {
    for (const link of links) {
      if (link.dataset.section === view) link.setAttribute('aria-current', 'location');
      else link.removeAttribute('aria-current');
    }
  }

  function filter() {
    const query = find.value.trim().toLocaleLowerCase();
    let visible = 0;
    for (const row of rows) {
      row.hidden = (group !== 'all' && row.dataset.group !== group)
        || !row.textContent.toLocaleLowerCase().includes(query);
      if (!row.hidden) visible++;
    }
    document.getElementById('result-count').textContent = `${visible} / ${rows.length}`;
    document.getElementById('empty-result').hidden = visible !== 0;
    for (const button of filters) button.setAttribute('aria-pressed', String(button.dataset.filter === group));
    status.textContent = `${visible} of ${rows.length} capabilities · ${group}${query ? ` / ${query}` : ''}`;
  }

  async function copy(text) {
    try {
      await navigator.clipboard.writeText(text);
      status.textContent = `Copied: ${text}`;
    } catch {
      status.textContent = 'Clipboard unavailable. Select the command text to copy it.';
    }
  }

  function followFragment() {
    const section = sections.find(item => '#' + item.id === location.hash);
    markSection(section ? section.dataset.panel : 'sessions');
  }
  window.addEventListener('hashchange', followFragment);
  for (const button of filters) button.addEventListener('click', () => {
    group = button.dataset.filter;
    filter();
  });
  for (const button of document.querySelectorAll('[data-copy]')) button.addEventListener('click', () => {
    copy(document.getElementById(button.dataset.copy).textContent.replace(/\s+/g, ' ').trim());
  });
  document.getElementById('copy-site').addEventListener('click', () => {
    copy(document.querySelector('link[rel="canonical"]').href);
  });
  find.addEventListener('input', filter);
  find.addEventListener('keydown', event => {
    if (event.key === 'Escape') {
      find.value = '';
      filter();
    }
  });
  document.addEventListener('keydown', event => {
    if (event.key === '/' && !event.ctrlKey && !event.metaKey && !event.altKey
        && !['INPUT', 'TEXTAREA'].includes(document.activeElement.tagName)) {
      event.preventDefault();
      find.focus();
    }
  });
  filter();
  followFragment();
  status.textContent = 'Persistent sessions · SSH · web tools · agent API';
})();
