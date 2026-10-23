(() => {
  // Читает уже открытую страницу каталога ДНС. Строки отдаёт как есть: числа разбирает программа, где это проверяется тестами.
  const clean = text => (text || '').replace(/\s+/g, ' ').trim();
  const textOf = (root, selector) => clean(root.querySelector(selector)?.innerText) || null;

  const page = clean(document.body?.innerText).slice(0, 2000);
  const items = [...document.querySelectorAll('.catalog-product[data-code]')];
  const captcha = items.length === 0 && /qrator|доступ (ограничен|запрещ)|проверк\S* браузер|подтвердите, что вы не робот|captcha/i.test(document.title + ' ' + page);
  const nothingFound = items.length === 0 && /ничего не найдено|не найдено ни одного/i.test(page);

  const cards = items
    .map(item => {
      const link = item.querySelector('.catalog-product__name');
      const href = link?.getAttribute('href');
      if (!href) return null;
      const url = new URL(href, location.origin);
      // В ссылке после названия — технические характеристики в квадратных скобках, они в название не входят.
      const title = clean((link.getAttribute('title') || link.innerText).replace(/\s*\[[^\]]*\]\s*$/, ''));
      const price = item.querySelector('.product-buy__price');
      const previous = price?.querySelector('.product-buy__prev');
      return {
        title,
        url: url.origin + url.pathname,
        in_stock: item.dataset.availStatus !== 'out_of_stock',
        price: price ? clean(price.innerText.replace(clean(previous?.innerText), '')) : null,
        old_price: clean(previous?.innerText) || null,
        rating: textOf(item, '.catalog-product__rating b'),
        reviews: textOf(item, '.catalog-product__rating'),
        place: textOf(item, '.order-avail-wrap_main'),
        // «Доставим на дом » — текстом, а сам срок («завтра») — в соседней кнопке: берём блок целиком.
        delivery: textOf(item, '.delivery-info-widget'),
      };
    })
    .filter(Boolean);

  // Цены ДНС подгружает после страницы: выдача готова, когда хотя бы у одного товара в наличии цена уже есть.
  // На document.readyState не смотрим: у ДНС он часто так и остаётся «interactive», хотя всё уже показано.
  const pricesReady = cards.some(card => card.price) || !cards.some(card => card.in_stock);
  // Запрос из одного бренда («makita») ДНС открывает как страницу бренда — там нет обычной выдачи.
  const brandPage = location.pathname.startsWith('/brand/');
  const state = captcha ? 'captcha'
    : nothingFound ? 'listings'
    : items.length > 0 && pricesReady ? 'listings'
    : brandPage && document.readyState !== 'loading' ? 'not_found'
    : 'loading';

  return {
    stale: !!window.__huntStale,
    state,
    // Куда ДНС перевёл поиск: следующие страницы — этот же адрес с номером страницы.
    url: location.href,
    // Суффикс класса («city-select__text_90n») ДНС меняет при обновлениях — ищем по началу имени.
    city: textOf(document, '[class*="city-select__text"]'),
    cards,
  };
})()
