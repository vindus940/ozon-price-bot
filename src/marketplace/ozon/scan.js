async (query, portions, parse, start) => {
  // Скачивает порции выдачи и отдаёт их разбору (parse.js) — сам карточки не разбирает.
  // start — адрес выдачи с фильтрами из ссылки слежки; без него ищем по словам query.
  const PAUSE_BETWEEN_PORTIONS_MS = 300;
  const MAX_REDIRECTS = 3;

  const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));

  let captcha = false;

  const fetchPortionJson = async (path, redirects = 0) => {
    const response = await fetch('/api/entrypoint-api.bx/page/json/v2?url=' + encodeURIComponent(path), { credentials: 'include' });
    if (response.status === 403) captcha = true;
    if (!response.ok) return null;
    const page = await response.json();
    if (page.redirectTo && redirects < MAX_REDIRECTS) {
      const target = new URL(page.redirectTo, location.origin);
      return fetchPortionJson(target.pathname + target.search, redirects + 1);
    }
    const widgets = Object.entries(page.widgetStates || {});
    return { items: parse.gridItems(widgets), next: parse.nextPortionPath(widgets) || page.nextPage || null };
  };

  const fetchPortionHtml = async path => {
    const response = await fetch(path, { credentials: 'include' });
    if (response.status === 403) captcha = true;
    if (!response.ok) return { items: [], next: null };
    const html = await response.text();
    const widgets = [...new DOMParser().parseFromString(html, 'text/html').querySelectorAll('[id^="state-"]')]
      .map(element => [element.id.replace(/^state-/, ''), element.getAttribute('data-state')]);
    const items = parse.gridItems(widgets);
    if (!items.length && /antibot|captcha|доступ ограничен|не робот/i.test(html)) captcha = true;
    return { items, next: parse.nextPortionPath(widgets) || null };
  };

  const fetchPortion = async path => {
    const portion = await fetchPortionJson(path).catch(() => null);
    if (portion?.items.length) return portion;
    return fetchPortionHtml(path).catch(() => ({ items: [], next: null }));
  };

  // Фильтры ссылки Ozon сам переносит в путь следующей порции; номер страницы — запасной путь, если листалки нет.
  const searchPath = page => {
    const first = start || '/search/?text=' + encodeURIComponent(query);
    return page > 1 ? first + (first.includes('?') ? '&' : '?') + 'page=' + page : first;
  };

  const offers = new Map();
  let path = searchPath(1);
  for (let portion = 1; portion <= portions; portion++) {
    const { items, next } = await fetchPortion(path);
    const before = offers.size;
    items
      .map(card => parse.toOffer(card, location.origin))
      .filter(Boolean)
      .forEach(offer => offers.has(offer.url) || offers.set(offer.url, offer));
    if (offers.size === before) break;
    path = next || searchPath(portion + 1);
    await sleep(PAUSE_BETWEEN_PORTIONS_MS);
  }

  return { captcha, offers: [...offers.values()] };
}
