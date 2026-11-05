(() => {
  // Читает открытую страницу поиска Wildberries. Строки отдаёт как есть: числа разбирает программа, где это проверяется
  // тестами. Классы оформления у WB со случайными суффиксами («price--TuQtK»), поэтому части карточки ищем по data-testid.
  const clean = text => (text || '').replace(/\s+/g, ' ').trim();
  const textOf = (root, testId) => clean(root.querySelector(`[data-testid="${testId}"]`)?.innerText) || null;

  const page = clean(document.body?.innerText).slice(0, 3000);
  // На пустой выдаче WB пишет «Ничего не нашлось по запросу …», но ниже показывает рекомендации с ценами — это не выдача.
  const nothingFound = /Ничего не нашлось по запросу/i.test(page);
  const items = nothingFound ? [] : [...document.querySelectorAll('.product-card[data-nm-id]')];
  // «Почти готово…» WB показывает на мгновение при каждой загрузке — это ещё загрузка, а не проверка «я не робот».
  const captcha = items.length === 0 && !nothingFound
    && /подтвердите, что вы не робот|доступ ограничен|captcha/i.test(document.title + ' ' + page);

  const cards = items.map(item => ({
    id: item.dataset.nmId,
    brand: textOf(item, 'productBrand'),
    name: textOf(item, 'productName'),
    price: textOf(item, 'product-card-current-price'),
    old_price: clean(item.querySelector('[data-testid="product-card-price"] del')?.innerText) || null,
    rating: textOf(item, 'catalogCardRating'),
    reviews: textOf(item, 'catalogCardCommentCount'),
    // На кнопке корзины WB пишет срок доставки: «1 ноября», «Завтра».
    delivery: textOf(item, 'order-button'),
  }));

  // Карточки дорисовываются после загрузки страницы: выдача готова, когда хотя бы у одной есть цена.
  const state = captcha ? 'captcha'
    : nothingFound ? 'listings'
    : cards.some(card => card.price) ? 'listings'
    : 'loading';

  return {
    stale: !!window.__huntStale,
    state,
    cards,
  };
})()
