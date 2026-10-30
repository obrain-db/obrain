/* ── SONDE DE GEL D'ÉCHELLE — À COLLER DANS LA CONSOLE DU NAVIGATEUR ────────
 *
 * Pourquoi cette sonde existe : le gel est intermittent, et il se produit sur la
 * machine qui a une vraie carte graphique — pas dans le navigateur de mesure de
 * l'agent (sans GPU), où toutes les images durent 17 à 67 ms quoi qu'il arrive.
 * Un gel « par moment » ne se laisse donc attraper que là où il a lieu.
 *
 * Ce qu'elle enregistre à chaque image, sans rien ralentir :
 *   · l'écart entre images, et les TÂCHES LONGUES (le navigateur les déclare) ;
 *   · les images d'animation longues AVEC la fonction qui les a bloquées ;
 *   · l'état de la bascule d'échelle au même instant (classe du `.stage`,
 *     opacité, voile, transformation) — un canvas noir par état coincé n'est pas
 *     un canvas qui rame ;
 *   · ce que le pilote a reçu (tampons créés, appels de dessin) ;
 *   · les requêtes en vol au moment du gel — un changement d'échelle qui attend
 *     une réponse ne se distingue pas d'un gel si personne ne le dit.
 *
 * USAGE : ouvrir le dashboard (84 32), ouvrir la console (F12), coller ce bloc
 * entier, valider. Reproduire le gel. Puis taper :  __gel.rapport()
 */
(() => {
  const IMAGE_LENTE = 80;          // ms au-delà desquelles une image nous intéresse
  const gel = { images: [], longues: [], loaf: [], requetes: [], debut: performance.now() };
  const t = () => performance.now() - gel.debut;

  // 1. écarts entre images
  let derniere = performance.now();
  const tick = () => {
    const n = performance.now(), d = n - derniere;
    if (d > IMAGE_LENTE) gel.images.push([Math.round(t()), Math.round(d), etat()]);
    derniere = n; requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);

  // 2. l'état de la bascule, au moment exact de l'image lente
  function etat() {
    const s = document.querySelector('.stage'), v = document.querySelector('.stage-veil');
    const cs = s && getComputedStyle(s);
    const cv = document.querySelector('canvas');
    return {
      stage: s ? s.className.replace('stage', '').trim() || '(aucune classe)' : 'pas de .stage',
      opacite: cs ? cs.opacity : '?', filtre: cs && cs.filter !== 'none' ? cs.filter : 'aucun',
      voile: v ? getComputedStyle(v).opacity : '?',
      canvas: cv ? cv.width + '×' + cv.height : 'absent',
    };
  }

  // 3. tâches longues + images d'animation longues (avec attribution)
  try { new PerformanceObserver((l) => { for (const e of l.getEntries()) gel.longues.push([Math.round(t()), Math.round(e.duration)]); }).observe({ entryTypes: ['longtask'] }); } catch (e) {}
  try {
    new PerformanceObserver((l) => {
      for (const e of l.getEntries()) {
        if (e.duration < IMAGE_LENTE) continue;
        gel.loaf.push([Math.round(t()), Math.round(e.duration), Math.round(e.blockingDuration || 0),
          (e.scripts || []).map((s) => `${s.sourceFunctionName || s.invoker || '?'} (${(s.sourceURL || '').split('/').pop()}:${s.sourceCharPosition ?? '?'}) ${Math.round(s.duration)}ms`)]);
      }
    }).observe({ type: 'long-animation-frame' });
  } catch (e) {}

  // 4. ce que le pilote reçoit, par seconde
  const C = window.WebGL2RenderingContext || window.WebGLRenderingContext;
  if (C) for (const [m, quoi] of [['createBuffer', 'tampons'], ['drawElements', 'dessins'], ['drawArrays', 'dessins'], ['texImage2D', 'textures']]) {
    const o = C.prototype[m]; if (!o) continue;
    C.prototype[m] = function (...a) { (gel[quoi] = (gel[quoi] || 0) + 1); return o.apply(this, a); };
  }

  // 5. les requêtes en vol : un changement d'échelle qui attend n'est pas un gel
  const origFetch = window.fetch;
  window.fetch = function (...a) {
    const url = String(a[0] && a[0].url ? a[0].url : a[0]);
    const d = t(); gel.requetes.push([Math.round(d), '→', url.replace(location.origin, '')]);
    return origFetch.apply(this, a).then((r) => { gel.requetes.push([Math.round(t()), '← ' + r.status, url.replace(location.origin, '')]); return r; });
  };

  gel.rapport = () => {
    const duree = Math.round(t() / 1000);
    console.log(`\n═══ SONDE DE GEL — ${duree} s d'enregistrement ═══`);
    console.log(`images > ${IMAGE_LENTE} ms : ${gel.images.length} · tâches longues : ${gel.longues.length} · images d'animation longues : ${gel.loaf.length}`);
    console.log(`pilote : ${gel.tampons || 0} tampons créés, ${gel.dessins || 0} appels de dessin, ${gel.textures || 0} textures` +
      (duree ? ` (≈ ${Math.round((gel.tampons || 0) / duree)} tampons/s)` : ''));
    console.log(`\n── LES 12 PIRES IMAGES, AVEC L'ÉTAT DE LA BASCULE À CET INSTANT ──`);
    for (const [ms, d, e] of gel.images.slice().sort((a, b) => b[1] - a[1]).slice(0, 12))
      console.log(`  t=${String(ms).padStart(6)} ms  image ${String(d).padStart(4)} ms  stage="${e.stage}" opacité=${e.opacite} filtre=${e.filtre} voile=${e.voile} canvas=${e.canvas}`);
    console.log(`\n── CE QUI A BLOQUÉ (images d'animation longues) ──`);
    for (const [ms, d, b, scripts] of gel.loaf.slice().sort((a, b2) => b2[1] - a[1]).slice(0, 8))
      console.log(`  t=${String(ms).padStart(6)} ms  image ${d} ms  blocage ${b} ms${scripts.length ? '  ← ' + scripts.slice(0, 3).join(' | ') : '  (aucun script : le temps est passé au rendu, pas au JavaScript)'}`);
    const requetesAutour = gel.requetes.filter(([ms]) => gel.images.some(([mi, d]) => ms >= mi - 300 && ms <= mi + d + 300));
    console.log(`\n── REQUÊTES PENDANT LES IMAGES LENTES (${requetesAutour.length}) ──`);
    for (const [ms, sens, u] of requetesAutour.slice(0, 16)) console.log(`  t=${String(ms).padStart(6)} ms  ${sens}  ${u.slice(0, 90)}`);
    return 'sonde : copiez tout ce qui précède.';
  };
  console.log('sonde installée — reproduisez le gel, puis tapez : __gel.rapport()');
  window.__gel = gel;
})();
