(() => {
  // Читает открытую страницу поиска Яндекс Маркета. Строки отдаёт как есть: числа разбирает программа, где это проверяется
  // тестами. Классы оформления у Маркета случайные, поэтому части карточки ищем по data-auto и data-zone-name.
  const clean = text => (text || '').replace(/\s+/g, ' ').trim();
  const textOf = (root, selector) => clean(root.querySelector(selector)?.innerText) || null;

  const captcha = location.pathname.startsWith('/showcaptcha') || /вы не робот/i.test(document.title);

  const cards = [...document.querySelectorAll('[data-zone-name="productSnippet"]')]
    .map(item => {
      const href = item.querySelector('[data-auto="snippet-link"]')?.getAttribute('href');
      if (!href) return null;
      const url = new URL(href, location.origin);
      return {
        // После пути — метки показа и рекламы, у каждого показа свои.
        url: url.origin + url.pathname,
        name: textOf(item, '[data-auto="snippet-title"]'),
        price: textOf(item, '[data-auto="snippet-price-current"]'),
        // «4.9 (25) · 26 купили»
        reviews: textOf(item, '[data-auto="reviews"]'),
        // «Завтра , ПВЗ По клику»
        delivery: textOf(item, '[data-zone-name="deliveryInfo"]'),
      };
    })
    .filter(Boolean);

  // Пустую выдачу Маркет подписывает, а бессмысленный запрос, наоборот, заполняет случайными товарами.
  const nothingFound = cards.length === 0 && /нет подходящих товаров/i.test(clean(document.body?.innerText));

  // Карточки дорисовываются после загрузки страницы: выдача готова, когда хотя бы у одной есть цена.
  const state = captcha ? 'captcha' : nothingFound || cards.some(card => card.price) ? 'listings' : 'loading';

  return {
    stale: !!window.__huntStale,
    state,
    url: location.href,
    cards,
  };
})()
