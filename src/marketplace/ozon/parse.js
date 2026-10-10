(() => {
  // Разбор выдачи Ozon: на входе — состояния виджетов страницы, на выходе — предложения.
  // Без сети, страницы и объектов браузера, чтобы тесты проверяли его на сохранённой выдаче.
  const parseJson = text => { try { return JSON.parse(text); } catch { return null; } };
  const clean = text => String(text).replace(/<[^>]*>/g, '').replace(/&nbsp;/g, ' ').replace(/\s+/g, ' ').trim();
  const integer = text => Number(text.replace(/ /g, ''));
  const rubles = text => {
    const match = clean(text).match(/^(\d[\d ]*)₽$/);
    return match ? integer(match[1]) : null;
  };

  // Виджеты называются «имя-номер-default-N», номер Ozon меняет при обновлениях — ищем по началу имени.
  const gridItems = widgets => widgets
    .filter(([name]) => /^(tileGrid|searchResults)/.test(name) && !/Sort|Filters/.test(name))
    .flatMap(([, state]) => parseJson(state)?.items || []);
  const nextPortionPath = widgets => widgets
    .filter(([name]) => /aginator/.test(name))
    .map(([, state]) => parseJson(state)?.nextPage)
    .find(Boolean);

  const nodesOf = card => {
    const nodes = [];
    const visit = node => {
      if (node && typeof node === 'object') {
        nodes.push(node);
        Object.values(node).forEach(visit);
      }
    };
    visit(card);
    return nodes;
  };
  const textsOf = nodes => nodes.flatMap(node => Object.values(node).filter(value => typeof value === 'string').map(clean));

  const nameOf = (nodes, texts) => {
    const atom = nodes
      .filter(node => node.id === 'name')
      .map(node => node[node.type] || node.textAtom || node.textDS)
      .find(candidate => candidate?.text);
    if (atom) return clean(atom.text);
    return texts.filter(text => text.length > 15 && !/₽|:\/\//.test(text)).sort((a, b) => b.length - a.length)[0] || '';
  };

  const pricesOf = (nodes, texts) => {
    const styled = style => nodes
      .filter(node => typeof node.text === 'string' && node.textStyle === style)
      .map(node => rubles(node.text))
      .find(Boolean) ?? null;
    const amounts = texts.map(rubles).filter(Boolean);
    const price = styled('PRICE') ?? (amounts.length ? Math.min(...amounts) : null);
    const oldPrice = styled('ORIGINAL_PRICE') ?? (amounts.length ? Math.max(...amounts) : null);
    return { price, oldPrice: oldPrice > price ? oldPrice : null };
  };

  // Строка рейтинга на плитке: «4.9», затем число отзывов без подписи — «8 834».
  const ratingOf = (nodes, texts) => {
    const labels = nodes
      .filter(node => node.testInfo?.automatizationId === 'tile-list-rating' && Array.isArray(node.items))
      .flatMap(node => node.items.map(item => item.text?.text))
      .filter(Boolean)
      .map(clean);
    const rating = labels.find(label => /^[1-5][.,]\d{1,2}$/.test(label));
    const reviews = labels.find(label => /^\d[\d ]*$/.test(label))
      ?? texts.map(text => text.match(/^(\d[\d ]*)отзыв/)?.[1]).find(Boolean);
    return {
      rating: rating ? parseFloat(rating.replace(',', '.')) : null,
      reviews: reviews ? integer(reviews) : null,
    };
  };

  // Ссылка без хвоста «?at=...»: иначе один товар из разных порций задваивался бы.
  const productUrl = (link, origin) => (/^https?:/.test(link) ? link : origin + link).replace(/[?#].*$/, '');

  const toOffer = (card, origin) => {
    const link = card?.action?.link;
    if (!link) return null;
    const nodes = nodesOf(card);
    const texts = textsOf(nodes);
    const { price, oldPrice } = pricesOf(nodes, texts);
    if (!price) return null;
    return {
      name: nameOf(nodes, texts),
      price,
      old_price: oldPrice,
      ...ratingOf(nodes, texts),
      url: productUrl(link, origin),
    };
  };

  return { gridItems, nextPortionPath, toOffer };
})()
