(() => {
  const clean = text => (text || '').replace(/\s+/g, ' ').trim();
  const textOf = (root, marker) => clean(root.querySelector(`[data-marker="${marker}"]`)?.innerText) || null;

  const captcha = /Доступ ограничен|проблема с IP/i.test(document.title + ' ' + clean(document.body?.innerText).slice(0, 1000));
  // Так Авито отвечает, например, на опечатку в городе: avito.ru/moskwa.
  const notFound = /Ошибка 404|Страница не найдена/i.test(document.title);
  const items = [...document.querySelectorAll('[data-marker="item"]')];
  // «Ничего не найдено в выбранной области поиска» — отдельный заголовок, вне блока выдачи.
  const nothingFound = [...document.querySelectorAll('h1, h2, h3')].some(heading => /Ничего не найдено/i.test(heading.innerText));

  const cards = items
    .map(item => {
      const link = item.querySelector('[data-marker="item-title"]');
      const href = link?.getAttribute('href');
      if (!href) return null;
      const url = new URL(href, location.origin);
      return {
        title: clean(link.innerText),
        url: url.origin + url.pathname,
        price: item.querySelector('[itemprop="price"]')?.getAttribute('content') || null,
        place: textOf(item, 'item-location'),
        posted: textOf(item, 'item-date'),
        seller_rating: textOf(item, 'seller-rating/score')
          || item.querySelector('[itemprop="ratingValue"]')?.getAttribute('content')
          || null,
        seller_reviews: textOf(item, 'seller-info/summary'),
        text: clean(item.innerText),
      };
    })
    .filter(Boolean);

  // «complete» на Авито наступает не всегда: на выдаче по ссылке с фильтрами часть картинок так и не догружается.
  // Объявления приходят в самой разметке, поэтому разобранная страница уже готова.
  const loaded = document.readyState !== 'loading';
  const state = !loaded ? 'loading'
    : captcha ? 'captcha'
    : notFound ? 'not_found'
    : items.length > 0 || nothingFound ? 'listings'
    : 'loading';

  // stale — это ещё старая страница: программа помечает её перед переходом.
  // path и radius — где страница оказалась на самом деле: Авито может перевести поиск в другой раздел.
  return {
    stale: !!window.__huntStale,
    state,
    path: location.pathname,
    radius: new URL(location.href).searchParams.get('radius'),
    cards,
  };
})()
