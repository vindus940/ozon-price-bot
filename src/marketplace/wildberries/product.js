(() => {
  // Читает открытую страницу товара Wildberries. Разметка у него разная: в браузере программы и в тестовом блоки цены
  // называются по-разному (5.11.2026), поэтому читаем не классы, а то, что есть всегда: заголовок вкладки
  // «Беспроводные наушники WH-CH520 SONY 880195976 купить за 4 294 ₽ в интернет-магазине Wildberries»
  // и суммы в тексте страницы. В заголовке — цена без кошелька, цена с кошельком стоит в тексте прямо перед ней.
  const clean = text => (text || '').replace(/\s+/g, ' ').trim();
  const rubles = text => Number(text.replace(/\D/g, ''));

  const page = clean(document.body?.innerText);
  const title = clean(document.title).match(/^(.+?)\s+\d+\s+купить(?:\s+за\s+(\d[\d ]*₽))?/i);
  // Сумма — группами по три цифры: иначе оценка «4,8» перед ценой склеится с ней в «8 4 165 ₽».
  const amounts = page.match(/\b\d{1,3}(?: \d{3})* ₽/g) || [];
  const regular = title?.[2] || null;
  const place = regular ? amounts.indexOf(regular) : -1;
  const before = place > 0 ? amounts[place - 1] : null;
  const withWallet = before && rubles(before) < rubles(regular) ? before : null;

  const start = page.slice(0, 3000);
  const captcha = !title && /подтвердите, что вы не робот|доступ ограничен|captcha/i.test(document.title + ' ' + start);
  const soldOut = !regular && /нет в наличии|распродан|товар закончился/i.test(start);
  const missing = !title && /страница не найдена|такой страницы нет/i.test(document.title + ' ' + start);
  const state = captcha ? 'captcha'
    : missing ? 'not_found'
    : title && (place >= 0 || soldOut) ? 'listings'
    : 'loading';

  return {
    stale: !!window.__huntStale,
    state,
    name: title?.[1] || '',
    price: withWallet || regular,
    price_without_wallet: withWallet ? regular : null,
  };
})()
