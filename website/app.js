(() => {
  const tabs = [...document.querySelectorAll('[data-tab]')];
  const panels = [...document.querySelectorAll('[data-panel]')];
  const index = [...document.querySelectorAll('[data-view]')];
  const status = document.getElementById('status-message');
  const find = document.getElementById('find');
  const rows = [...document.querySelectorAll('#capabilities tbody tr')];
  const filters = [...document.querySelectorAll('[data-filter]')];
  let group = 'all';

  function select(view, focus = false) {
    for (const tab of tabs) {
      const active = tab.dataset.tab === view;
      tab.setAttribute('aria-selected', String(active));
      tab.tabIndex = active ? 0 : -1;
      if (active && focus) tab.focus();
    }
    for (const panel of panels) panel.hidden = panel.dataset.panel !== view;
    for (const item of index) item.setAttribute('aria-current', String(item.dataset.view === view));
    status.textContent = `Explore ${view} · / to find a capability`;
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

  for (const tab of tabs) {
    tab.addEventListener('click', () => select(tab.dataset.tab));
    tab.addEventListener('keydown', event => {
      const current = tabs.indexOf(tab);
      const next = { ArrowRight: (current + 1) % tabs.length,
        ArrowLeft: (current + tabs.length - 1) % tabs.length, Home: 0, End: tabs.length - 1 }[event.key];
      if (next === undefined) return;
      event.preventDefault();
      select(tabs[next].dataset.tab, true);
    });
  }
  for (const item of index) item.addEventListener('click', () => select(item.dataset.view));
  for (const button of filters) button.addEventListener('click', () => {
    group = button.dataset.filter;
    filter();
  });
  for (const button of document.querySelectorAll('[data-copy]')) button.addEventListener('click', () => {
    copy(document.getElementById(button.dataset.copy).textContent);
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
  select('sessions');
})();
