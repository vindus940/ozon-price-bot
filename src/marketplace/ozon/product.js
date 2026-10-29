async (path, parse) => {
  // Страница товара приходит тем же запросом из вкладки, что и выдача; разбирает её parse.js (productOf).
  const MAX_REDIRECTS = 3;

  for (let redirects = 0; redirects <= MAX_REDIRECTS; redirects++) {
    const response = await fetch('/api/entrypoint-api.bx/page/json/v2?url=' + encodeURIComponent(path), { credentials: 'include' });
    if (response.status === 403) return { captcha: true, product: null };
    if (!response.ok) return { captcha: false, product: null };
    const page = await response.json();
    if (!page.redirectTo) {
      return { captcha: false, product: parse.productOf(Object.entries(page.widgetStates || {})) };
    }
    const target = new URL(page.redirectTo, location.origin);
    path = target.pathname + target.search;
  }
  return { captcha: false, product: null };
}
