(() => {
  // Читает открытую страницу товара ДНС. Цену и наличие берёт из описания товара для поисковиков (schema.org Product):
  // оно приходит в самой разметке, а классы оформления ДНС меняет при обновлениях.
  const clean = text => (text || '').replace(/\s+/g, ' ').trim();

  const page = clean(document.body?.innerText).slice(0, 2000);
  const described = [...document.querySelectorAll('script[type="application/ld+json"]')]
    .map(script => { try { return JSON.parse(script.textContent); } catch { return null; } })
    .find(data => data?.['@type'] === 'Product');
  const offer = [described?.offers].flat().find(Boolean);
  const captcha = !described && /qrator|доступ (ограничен|запрещ)|проверк\S* браузер|подтвердите, что вы не робот|captcha/i.test(document.title + ' ' + page);
  const missing = !described && document.readyState !== 'loading' && /не найден|404/i.test(document.title);
  const state = captcha ? 'captcha'
    : described && offer ? 'listings'
    : missing ? 'not_found'
    : 'loading';

  return {
    stale: !!window.__huntStale,
    state,
    name: clean(described?.name),
    price: Math.round(Number(offer?.price)) || null,
    in_stock: /InStock/.test(offer?.availability || ''),
    // Суффикс класса («city-select__text_90n») ДНС меняет при обновлениях — ищем по началу имени.
    city: clean(document.querySelector('[class*="city-select__text"]')?.innerText) || null,
  };
})()
