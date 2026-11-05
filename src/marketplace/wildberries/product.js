(() => {
  // Читает открытую страницу товара Wildberries. Классы у WB со случайными суффиксами, поэтому блок цены ищем по началу
  // имени. Первая сумма в блоке — цена, с которой товар продаётся; если блок с кошельком, вторая — цена без него.
  const clean = text => (text || '').replace(/\s+/g, ' ').trim();

  const page = clean(document.body?.innerText).slice(0, 3000);
  const block = document.querySelector('[class*="priceBlockPriceWrap"]');
  const amounts = clean(block?.innerText).match(/\d[\d\s]*₽/g) || [];
  const withWallet = !!document.querySelector('[class*="priceBlockPriceWrapWallet"]');
  const captcha = !block && /подтвердите, что вы не робот|доступ ограничен|captcha/i.test(document.title + ' ' + page);
  const soldOut = !block && /нет в наличии|распродан|товар закончился/i.test(page);
  const missing = !block && /страница не найдена|такой страницы нет/i.test(document.title + ' ' + page);
  // Заголовок вкладки WB меняет позже цены: сначала там общий текст магазина, поэтому имя — из карточки.
  const name = clean(document.querySelector('[data-testid="product-line-name"]')?.innerText);
  const state = captcha ? 'captcha'
    : missing ? 'not_found'
    : name && (amounts.length > 0 || soldOut) ? 'listings'
    : 'loading';

  return {
    stale: !!window.__huntStale,
    state,
    name,
    price: amounts[0] || null,
    price_without_wallet: withWallet ? amounts[1] || null : null,
  };
})()
