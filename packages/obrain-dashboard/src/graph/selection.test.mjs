/**
 * TESTS DE LA SÉLECTION PAR POINTAGE — Node pur, AUCUN framework, aucune
 * dépendance (pas de three.js : les matrices de vue sont construites ici, en
 * colonne-major, exactement comme three.js les pose).
 *
 * Exécution (celle de l'énoncé, telle quelle) :
 *
 *   cd packages/obrain-dashboard
 *   npx tsc src/graph/selection.ts --outDir /tmp/sel-test --module esnext \
 *     --target es2020 --moduleResolution bundler --strict
 *   node src/graph/selection.test.mjs
 *
 * Le test importe le JS COMPILÉ (le module est du TypeScript pur, sans import :
 * il n'a pas besoin d'être bundlé pour être exécuté).
 */

import assert from "node:assert/strict";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

// ---------------------------------------------------------------------------
// 0. Où est le module compilé ?
// ---------------------------------------------------------------------------
//
// Deux détails qui font échouer une version naïve (mêmes que dans
// `cadrage.test.mjs`) :
//   · `/tmp/sel-test` n'a pas de `package.json` : sans marqueur, Node peut lire
//     `selection.js` comme du CommonJS et refuser ses `export` (Node ≥ 22 le
//     devine, les versions antérieures non). On pose donc le marqueur AVANT
//     d'importer — seul fichier écrit par ce test, et HORS du dépôt ;
//   · `tsc` déduit sa racine commune du seul fichier d'entrée, mais elle peut
//     changer selon la version : on essaie les emplacements possibles au lieu
//     d'en supposer un.
const SORTIE = process.env.SEL_TEST_OUT ?? "/tmp/sel-test";
mkdirSync(SORTIE, { recursive: true });
writeFileSync(join(SORTIE, "package.json"), '{ "type": "module" }\n');

const PISTES = [
  process.env.SEL_JS,
  join(SORTIE, "selection.js"),
  join(SORTIE, "graph", "selection.js"),
  join(SORTIE, "src", "graph", "selection.js"),
  new URL("./selection.js", import.meta.url).pathname,
].filter((p) => typeof p === "string" && p.length > 0);

const CHEMIN = PISTES.find((p) => existsSync(p));
if (!CHEMIN) {
  console.error(`Module compilé introuvable — attendu l'un de : ${PISTES.join(", ")}`);
  console.error("Compilez d'abord :");
  console.error("  npx tsc src/graph/selection.ts --outDir /tmp/sel-test --module esnext --target es2020 --moduleResolution bundler --strict");
  process.exit(1);
}
const sel = await import(pathToFileURL(CHEMIN).href);
const { projeter, empreintePx, classer, choisir, REGLAGES_DEFAUT } = sel;

// ---------------------------------------------------------------------------
// 1. Matrices colonne-major (conventions three.js), sans three.js.
// ---------------------------------------------------------------------------
const perspective = (fovDeg, aspect, near, far) => {
  const t = 1 / Math.tan((fovDeg * Math.PI) / 360);
  const e = new Array(16).fill(0);
  e[0] = t / aspect;
  e[5] = t;
  e[10] = -(far + near) / (far - near);
  e[11] = -1;
  e[14] = (-2 * far * near) / (far - near);
  return e;
};
const sub = (a, b) => [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
const croix = (a, b) => [
  a[1] * b[2] - a[2] * b[1],
  a[2] * b[0] - a[0] * b[2],
  a[0] * b[1] - a[1] * b[0],
];
const norme = (a) => {
  const l = Math.hypot(a[0], a[1], a[2]) || 1;
  return [a[0] / l, a[1] / l, a[2] / l];
};
/**
 * Matrice de VUE (colonne-major). Attention au piège : `Matrix4.lookAt` de
 * three.js ne pose QUE la rotation — la translation d'une vraie matrice de vue
 * est −R·œil, que three.js obtient en inversant `matrixWorld`. On la pose donc
 * ici explicitement (sinon tout point se projette derrière la caméra).
 */
const vue = (oeil, cible, haut = [0, 1, 0]) => {
  const z = norme(sub(oeil, cible));
  const x = norme(croix(haut, z));
  const y = croix(z, x);
  const d = (v) => -(v[0] * oeil[0] + v[1] * oeil[1] + v[2] * oeil[2]);
  return [
    x[0], x[1], x[2], 0,
    y[0], y[1], y[2], 0,
    z[0], z[1], z[2], 0,
    d(x), d(y), d(z), 1,
  ];
};
/** Produit a·b, colonne-major : (r,c) = e[c*4+r]. */
const mul = (a, b) => {
  const t = new Array(16).fill(0);
  for (let c = 0; c < 4; c += 1) {
    for (let r = 0; r < 4; r += 1) {
      let s = 0;
      for (let k = 0; k < 4; k += 1) s += a[k * 4 + r] * b[c * 4 + k];
      t[c * 4 + r] = s;
    }
  }
  return t;
};

const FOV = 60;
const NEAR = 1;
const FAR = 12000;
/** Caméra de la vue Cosmos : fov 60, near 1, far 12000 (grammar.ts, ligne 72). */
const camera = (oeil, cible, { largeur = 1000, hauteur = 600, haut = [0, 1, 0] } = {}) => ({
  position: { x: oeil[0], y: oeil[1], z: oeil[2] },
  viewProjection: mul(perspective(FOV, largeur / hauteur, NEAR, FAR), vue(oeil, cible, haut)),
  largeur,
  hauteur,
});

const H = 600;
const W = 1000;
const TAN = Math.tan((FOV * Math.PI) / 360);
/** Pixels par unité monde à la distance d (loi du dépôt : 2·tan(fov/2)·d/hauteur). */
const pxParUnite = (d) => H / (2 * TAN * d);
/** Décalage monde, à la distance d, qui vaut n pixels à l'écran. */
const mondePourPx = (d, n) => n / pxParUnite(d);
/** Espacement minimal entre deux `cas()` imprimés côte à côte. */
const proche = (a, b, tol) => Math.abs(a - b) <= tol;

// ---------------------------------------------------------------------------
// 2. Micro-harnais (aucun framework) : un cas = un nom + une fonction.
// ---------------------------------------------------------------------------
let total = 0;
let ko = 0;
const cas = (nom, fn) => {
  total += 1;
  try {
    fn();
    console.log(`  ok   ${nom}`);
  } catch (e) {
    ko += 1;
    console.log(`  KO   ${nom}`);
    console.log(`       ${String(e && e.message).split("\n")[0]}`);
  }
};
const titre = (t) => console.log(`\n── ${t} ${"─".repeat(Math.max(0, 66 - t.length))}`);

// Candidats d'essai ---------------------------------------------------------
const noeud = (ancre, o = {}) => ({
  genre: "noeud",
  qn: "graphe:monde",
  nom: "src/a.ts::f",
  ancre: { x: ancre[0], y: ancre[1], z: ancre[2] },
  rayon: 0,
  opacite: 0.9,
  etages: [2],
  focal: true,
  ...o,
});
const monde = (ancre, rayon, o = {}) => ({
  genre: "monde",
  qn: "monde:x",
  nom: "x",
  ancre: { x: ancre[0], y: ancre[1], z: ancre[2] },
  rayon,
  opacite: 0.9,
  etages: [0, 1],
  focal: false,
  ...o,
});
const ensemble = (ancre, rayon, o = {}) => ({
  genre: "ensemble",
  qn: "monde:x",
  nom: "src",
  ancre: { x: ancre[0], y: ancre[1], z: ancre[2] },
  rayon,
  opacite: 0.55,
  etages: [2],
  focal: true,
  ...o,
});

console.log(`Sélection — module compilé : ${CHEMIN}`);
console.log(`Réglages par défaut : ${JSON.stringify(REGLAGES_DEFAUT)}`);

// ---------------------------------------------------------------------------
// 3. LA PROJECTION — la base de tout : si elle est fausse, tout est faux.
// ---------------------------------------------------------------------------
titre("projection (px écran, three.js colonne-major)");

cas("la cible est au centre de l'écran", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const p = projeter(cam, { x: 0, y: 0, z: 0 });
  assert.ok(p, "la cible doit se projeter");
  assert.ok(proche(p.x, W / 2, 1e-6) && proche(p.y, H / 2, 1e-6), `${p.x},${p.y}`);
});

cas("la demi-hauteur du tronc vaut tan(fov/2)·d : bord haut à y = 0", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const yBord = TAN * 1200; // 692,82 unités monde à 1200
  const p = projeter(cam, { x: 0, y: yBord, z: 0 });
  assert.ok(p && proche(p.y, 0, 1e-6), `y = ${p && p.y}`);
});

cas("+X monde va vers la droite de l'écran", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const p = projeter(cam, { x: mondePourPx(1200, 100), y: 0, z: 0 });
  assert.ok(p && proche(p.x, W / 2 + 100, 1e-6), `x = ${p && p.x}`);
});

cas("un point derrière la caméra rend null", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  assert.equal(projeter(cam, { x: 0, y: 0, z: 1300 }), null);
});

// ---------------------------------------------------------------------------
// 4. L'EMPREINTE — la grandeur qui manquait. Elle doit suivre la loi du dépôt.
// ---------------------------------------------------------------------------
titre("empreinte apparente (px)");

cas("monde de rayon 20 à 1200 unités : 8,66 px (loi du dépôt, tolérance 0,5 %)", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const e = empreintePx(cam, { x: 0, y: 0, z: 0 }, 20);
  assert.ok(e, "empreinte attendue");
  const attendu = (20 * H) / (2 * TAN * 1200);
  assert.ok(proche(e.empreinte, attendu, attendu * 0.005), `${e.empreinte} contre ${attendu}`);
});

cas("elle DIVISE par la distance : ×10 plus loin, ÷10 plus petite", () => {
  const procheCam = camera([0, 0, 600], [0, 0, 0]);
  const loinCam = camera([0, 0, 6000], [0, 0, 0]);
  const a = empreintePx(procheCam, { x: 0, y: 0, z: 0 }, 200);
  const b = empreintePx(loinCam, { x: 0, y: 0, z: 0 }, 200);
  assert.ok(a && b);
  assert.ok(proche(b.empreinte * 10, a.empreinte, a.empreinte * 0.01), `${a.empreinte} / ${b.empreinte}`);
});

cas("sous-estimation mesurée : √(1−(r/d)²) — 1,1 % à r/d = 0,15", () => {
  const cam = camera([0, 0, 1000], [0, 0, 0]);
  const r = 150; // r/d = 0,15
  const e = empreintePx(cam, { x: 0, y: 0, z: 0 }, r);
  assert.ok(e);
  const exacte = (H / 2) * Math.tan(Math.asin(r / 1000)) / TAN;
  const rapport = e.empreinte / exacte;
  assert.ok(proche(rapport, Math.sqrt(1 - 0.15 ** 2), 0.002), `rapport ${rapport}`);
  assert.ok(rapport < 1, "la mesure ne doit jamais majorer la silhouette");
});

cas("rayon nul → empreinte nulle (un nœud est un point)", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const e = empreintePx(cam, { x: 0, y: 0, z: 0 }, 0);
  assert.ok(e && e.empreinte === 0);
});

cas("REG RÉGRESSION : vu du pôle, un décalage +Y mesurerait ZÉRO", () => {
  // Caméra à la verticale d'une boule de rayon 100 à 1000 unités. Le décalage
  // « +Y » (celui de `rayonEcran`, CosmosWeave ligne 3610) est alors confondu
  // avec l'axe optique : les deux points se projettent au MÊME endroit — la
  // boule est vue comme un point. Le décalage perpendiculaire, lui, mesure.
  const cam = camera([0, 1000, 0], [0, 0, 0], { haut: [0, 0, 1] });
  const centre = projeter(cam, { x: 0, y: 0, z: 0 });
  const decaleY = projeter(cam, { x: 0, y: 100, z: 0 });
  const aveugle = Math.hypot(decaleY.x - centre.x, decaleY.y - centre.y);
  const nôtre = empreintePx(cam, { x: 0, y: 0, z: 0 }, 100);
  assert.ok(nôtre);
  assert.ok(aveugle < 0.5, `décalage +Y = ${aveugle} px (attendu ≈ 0)`);
  assert.ok(proche(nôtre.empreinte, (100 * H) / (2 * TAN * 1000), 0.5), `${nôtre.empreinte} px`);
});

// ---------------------------------------------------------------------------
// 5. LE RAYON DE SÉLECTION EST UNE LOI EN PIXELS.
// ---------------------------------------------------------------------------
titre("le rayon de sélection ne dépend plus de l'échelle");

cas("un nœud garde 6 px de rayon de sélection à 100, 1200 et 4000 unités", () => {
  const rayons = [100, 1200, 4000].map((d) => {
    const cam = camera([0, 0, d], [0, 0, 0]); // l'origine est donc à d unités
    const cl = classer([noeud([0, 0, 0])], cam, { x: W / 2, y: H / 2 }, 2);
    return cl[0].rayonPx;
  });
  assert.deepEqual(rayons, [6, 6, 6], JSON.stringify(rayons));
});

cas("l'ANCIEN seuil (7 unités) variait de 40× : 36,4 px à 100, 0,9 px à 4000", () => {
  const px = [100, 1200, 4000].map((d) => 7 * pxParUnite(d));
  assert.ok(px[0] / px[2] > 30, `${px.map((v) => v.toFixed(2)).join(" | ")}`);
  assert.ok(px[2] < 1, "de loin, l'ancien seuil était sous le pixel");
});

// ---------------------------------------------------------------------------
// 6. UNE BOULE ÉNORME VUE DE LOIN : grande empreinte, rayon de sélection COHÉRENT.
// ---------------------------------------------------------------------------
titre("boule énorme de loin (rayon monde 900 à 6000 unités)");

cas("empreinte ≈ 78 px (et non 7 unités ≈ 0,6 px)", () => {
  const cam = camera([0, 0, 6000], [0, 0, 0]);
  const e = empreintePx(cam, { x: 0, y: 0, z: 0 }, 900);
  assert.ok(e);
  assert.ok(e.empreinte > 50, `empreinte ${e.empreinte} px`);
  assert.ok(proche(e.empreinte, (900 * H) / (2 * TAN * 6000), 0.5), `${e.empreinte} px`);
  assert.ok(7 * pxParUnite(6000) < 1, "l'ancien seuil en unités monde valait moins d'un pixel");
});

cas("rayon de sélection = 2,2 × empreinte, PLAFONNÉ (jamais 7 unités)", () => {
  const cam = camera([0, 0, 6000], [0, 0, 0]);
  const cl = classer([monde([0, 0, 0], 900, { focal: true, etages: [0] })], cam, { x: W / 2, y: H / 2 }, 0);
  assert.equal(cl[0].rayonPx, 160, "plafond");
  assert.ok(cl[0].rayonPx > 7, "le plancher/plafond sont en PIXELS");
  assert.ok(cl[0].score > 0);
});

cas("on la sélectionne à 60 px du centre, plus à 400 px", () => {
  const cam = camera([0, 0, 6000], [0, 0, 0]);
  const cs = [monde([0, 0, 0], 900, { focal: true, etages: [0] })];
  const pres = choisir(cs, cam, { x: W / 2 + 60, y: H / 2 }, 0);
  const loin = choisir(cs, cam, { x: W / 2 + 400, y: H / 2 }, 0);
  assert.ok(pres, "60 px : dans l'empreinte (78 px) → désignable");
  assert.equal(pres.px, 0, "le pointeur est SUR l'objet");
  assert.equal(loin, null, "400 px > portée (238 px) → rien");
});

// ---------------------------------------------------------------------------
// 7. L'ÉTAGE PRIME.
// ---------------------------------------------------------------------------
titre("priorité d'étage");

cas("nœud à 3 px (étage 2) contre monde à 1 px (étages 0-1) : le nœud gagne", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const pointeur = { x: W / 2 + 1, y: H / 2 }; // monde à 1 px, nœud à 3 px
  const cs = [
    monde([0, 0, 0], 20, { focal: false }),
    noeud([mondePourPx(1200, 4), 0, 0], { etages: [2], focal: true }),
  ];
  const gagnant = choisir(cs, cam, pointeur, 2);
  assert.ok(gagnant, "un candidat doit gagner");
  assert.equal(gagnant.candidat.genre, "noeud");
  assert.ok(proche(gagnant.px, 3, 1e-6), `px = ${gagnant.px}`);

  const cl = classer(cs, cam, pointeur, 2);
  const m = cl.find((x) => x.candidat.genre === "monde");
  assert.ok(m.score > 0, "le monde reste désignable (repli), il n'est pas ignoré");
  assert.ok(gagnant.score > m.score, `${gagnant.score} > ${m.score}`);
  assert.equal(cl[0].candidat.genre, "noeud", "le classement est trié, meilleur d'abord");
  assert.equal(cl[0].score, gagnant.score);
});

cas("le même couple à l'étage 0 : le monde (focal) gagne — l'étage décide, pas le genre", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const pointeur = { x: W / 2 + 1, y: H / 2 };
  const cs = [
    monde([0, 0, 0], 20, { focal: true }), // étages [0,1], focal
    noeud([mondePourPx(1200, 4), 0, 0], { etages: [2], focal: true }),
  ];
  const gagnant = choisir(cs, cam, pointeur, 0);
  assert.ok(gagnant);
  assert.equal(gagnant.candidat.genre, "monde");
});

cas("INVARIANT : un focal au BORD de sa portée bat un non-focal pile dessus", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const cs = [
    noeud([mondePourPx(1200, 6), 0, 0], { etages: [2], focal: true }), // à 6 px = sa portée
    noeud([0, 0, 0], { etages: [2], focal: false }), // pile au pointeur
  ];
  const cl = classer(cs, cam, { x: W / 2, y: H / 2 }, 2);
  const focal = cl.find((x) => x.candidat.focal);
  const nonFocal = cl.find((x) => !x.candidat.focal);
  assert.ok(focal.score > nonFocal.score, `${focal.score} > ${nonFocal.score}`);
  assert.ok(proche(nonFocal.px, 0, 1e-6));
  assert.ok(proche(focal.px, 6, 1e-6));
});

// ---------------------------------------------------------------------------
// 8. LA CAGE QUASI TRANSPARENTE (le cas motivant).
// ---------------------------------------------------------------------------
titre("cage de monde effacée");

cas("opacité 0,10 : non désignable, même si elle contient tout le pointeur", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const cage = monde([0, 0, 0], 40, { opacite: 0.1, etages: [0, 1], focal: false });
  const groupe = ensemble([mondePourPx(1200, 6), 0, 0], 8);
  const gagnant = choisir([cage, groupe], cam, { x: W / 2, y: H / 2 }, 2);
  assert.ok(gagnant, "le groupe focalisé doit gagner");
  assert.equal(gagnant.candidat.genre, "ensemble");

  const cl = classer([cage, groupe], cam, { x: W / 2, y: H / 2 }, 2);
  const c = cl.find((x) => x.candidat.genre === "monde");
  assert.equal(c.px, 0, "le pointeur est bien SUR la cage (empreinte 17,3 px)");
  assert.equal(c.score, 0, "mais son opacité la disqualifie");
  assert.ok(c.raisons.join(" ").includes("opacité 0.10"), c.raisons.join(" | "));
  assert.ok(c.raisons.join(" ").includes("NON DÉSIGNABLE"));
  assert.ok(cl[0].score > c.score);
});

cas("opacité 0,60 : désignable mais PÉNALISÉE, exactement du rapport des visibilités", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const a = classer([monde([0, 0, 0], 40, { opacite: 0.6, focal: false })], cam, { x: W / 2, y: H / 2 }, 2)[0];
  const b = classer([monde([0, 0, 0], 40, { opacite: 0.95, focal: false })], cam, { x: W / 2, y: H / 2 }, 2)[0];
  assert.ok(a.score > 0 && b.score > 0);
  // Mêmes empreinte, proximité et étage : seul `vis` change → le rapport des
  // scores est EXACTEMENT le rapport des visibilités (0,9625 / 0,700).
  assert.ok(proche(b.score / a.score, (0.25 + 0.75 * 0.95) / (0.25 + 0.75 * 0.6), 0.01),
    `${b.score} / ${a.score}`);
  assert.ok(a.raisons.join(" ").includes("visibilité ×0.700"));
});

// ---------------------------------------------------------------------------
// 9. LA FACE ARRIÈRE : depuis l'intérieur d'un monde.
// ---------------------------------------------------------------------------
titre("face arrière (caméra dans la boule)");

cas("depuis l'intérieur, la cage ne vole plus la place au groupe", () => {
  const cam = camera([0, 0, 100], [0, 0, 0]); // 100 unités du centre
  const cage = monde([0, 0, 0], 400, { opacite: 0.6, etages: [0, 1], focal: false });
  const dedans = noeud([mondePourPx(100, 3), 0, 0], { etages: [2], focal: true });
  const cl = classer([cage, dedans], cam, { x: W / 2, y: H / 2 }, 2);
  assert.equal(cl[0].candidat.genre, "noeud");
  const c = cl.find((x) => x.candidat.genre === "monde");
  assert.ok(c.raisons.join(" ").includes("DANS la boule"), c.raisons.join(" | "));
  assert.ok(c.score > 0 && c.score < 50, `score ${c.score} : pénalisée, pas ignorée`);
});

cas("mais jamais ignorée : seule, elle reste désignable (le repli existe)", () => {
  const cam = camera([0, 0, 100], [0, 0, 0]);
  const cage = monde([0, 0, 0], 400, { opacite: 0.6, etages: [0], focal: true });
  const g = choisir([cage], cam, { x: W / 2, y: H / 2 }, 0);
  assert.ok(g, "un clic dans un monde vide doit encore faire quelque chose");
  assert.equal(g.candidat.genre, "monde");
});

// ---------------------------------------------------------------------------
// 10. LES CAS DE REFUS.
// ---------------------------------------------------------------------------
titre("refus explicites");

cas("un point derrière la caméra est ignoré (choisir → null)", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const derriere = noeud([0, 0, 1300]);
  assert.equal(projeter(cam, derriere.ancre), null);
  assert.equal(choisir([derriere], cam, { x: W / 2, y: H / 2 }, 2), null);
  const cl = classer([derriere], cam, { x: W / 2, y: H / 2 }, 2);
  assert.equal(cl.length, 1, "il reste tracé dans le classement");
  assert.equal(cl[0].score, 0);
  assert.equal(cl[0].px, Infinity, "distance non mesurable");
  assert.ok(cl[0].raisons.join(" ").includes("DERRIÈRE"));
});

cas("aucun candidat dans le rayon → null", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const loin = noeud([mondePourPx(1200, 40), 0, 0]); // 40 px, portée 6 px
  assert.equal(choisir([loin], cam, { x: W / 2, y: H / 2 }, 2), null);
  const cl = classer([loin], cam, { x: W / 2, y: H / 2 }, 2);
  assert.equal(cl[0].score, 0);
  assert.ok(cl[0].raisons.join(" ").includes("hors de portée"), cl[0].raisons.join(" | "));
});

cas("tableau vide → [] et null", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  assert.deepEqual(classer([], cam, { x: 0, y: 0 }, 2), []);
  assert.equal(choisir([], cam, { x: 0, y: 0 }, 2), null);
  assert.deepEqual(classer(null, cam, { x: 0, y: 0 }, 2), []);
});

cas("coordonnées non finies : le candidat est ignoré (absent du classement)", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const cs = [
    noeud([Number.NaN, 0, 0]),
    noeud([0, 0, 0], { rayon: Number.POSITIVE_INFINITY }),
    noeud([0, 0, 0], { opacite: undefined }),
    { ...noeud([0, 0, 0]), ancre: undefined },
    noeud([0, 0, 0], { nom: "valide" }),
  ];
  const cl = classer(cs, cam, { x: W / 2, y: H / 2 }, 2);
  assert.equal(cl.length, 1, JSON.stringify(cl.map((x) => x.candidat.nom)));
  assert.equal(cl[0].candidat.nom, "valide");
});

cas("aucune valeur non finie ne sort du classement", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const cl = classer(
    [noeud([0, 0, 0]), monde([0, 0, 0], 40, { focal: true, opacite: 0.2 }), noeud([0, 0, 1300])],
    cam,
    { x: W / 2, y: H / 2 },
    2,
  );
  for (const c of cl) {
    assert.ok(Number.isFinite(c.score), `score ${c.score}`);
    assert.ok(Number.isFinite(c.rayonPx), `rayonPx ${c.rayonPx}`);
    assert.ok(c.px === Infinity || Number.isFinite(c.px), `px ${c.px}`);
    assert.ok(Array.isArray(c.raisons) && c.raisons.length >= 1, "chaque candidat est motivé");
    // Un candidat désignable porte TOUJOURS la ligne qui détaille son score.
    if (c.score > 0) assert.ok(c.raisons.join(" ").includes("score"), c.raisons.join(" | "));
  }
});

cas("caméra inutilisable ou pointeur non fini → aucun classement", () => {
  const bonne = camera([0, 0, 1200], [0, 0, 0]);
  const cs = [noeud([0, 0, 0])];
  assert.deepEqual(classer(cs, { ...bonne, largeur: 0 }, { x: 0, y: 0 }, 2), []);
  assert.deepEqual(classer(cs, { ...bonne, viewProjection: [1, 2, 3] }, { x: 0, y: 0 }, 2), []);
  assert.deepEqual(classer(cs, { ...bonne, position: undefined }, { x: 0, y: 0 }, 2), []);
  const cassee = { ...bonne, viewProjection: [...bonne.viewProjection] };
  cassee.viewProjection[5] = Number.NaN;
  assert.deepEqual(classer(cs, cassee, { x: 0, y: 0 }, 2), []);
  assert.deepEqual(classer(cs, bonne, { x: Number.NaN, y: 0 }, 2), []);
  assert.equal(choisir(cs, { ...bonne, largeur: 0 }, { x: 0, y: 0 }, 2), null);
});

cas("réglages absurdes (0, négatifs, NaN) : ramenés aux défauts, jamais de NaN", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const cl = classer([noeud([0, 0, 0])], cam, { x: W / 2, y: H / 2 }, 2, {
    plancherPx: 0,
    facteur: -3,
    plafondPx: Number.NaN,
    seuilOpacite: 42,
    bonusFocal: Number.NaN,
  });
  assert.equal(cl[0].rayonPx, REGLAGES_DEFAUT.plancherPx);
  assert.ok(Number.isFinite(cl[0].score) && cl[0].score > 0);
});

// ---------------------------------------------------------------------------
// 11. TRACABILITÉ : `raisons` dit tout ce qui a été appliqué.
// ---------------------------------------------------------------------------
titre("traçabilité");

cas("tous les facteurs sont nommés, formule comprise", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const cl = classer([ensemble([0, 0, 0], 8)], cam, { x: W / 2, y: H / 2 }, 2)[0];
  const t = cl.raisons.join(" | ");
  for (const attendu of ["empreinte", "rayon de sélection", "proximité", "précision", "opacité", "étage 2", "score"]) {
    assert.ok(t.includes(attendu), `« ${attendu} » manque dans : ${t}`);
  }
  assert.ok(t.includes("score") && t.includes("×"), t);
});

cas("l'étage courant est reconnu, et son absence est dite « repli »", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const legitime = classer([ensemble([0, 0, 0], 8)], cam, { x: W / 2, y: H / 2 }, 2)[0];
  const repli = classer([ensemble([0, 0, 0], 8)], cam, { x: W / 2, y: H / 2 }, 0)[0];
  assert.ok(legitime.raisons.join(" ").includes("étage 2 légitime et focal"));
  assert.ok(repli.raisons.join(" ").includes("hors de [2]"));
  assert.ok(repli.raisons.join(" ").includes("repli ×0.20"));
  assert.ok(legitime.score > repli.score * 10, `${legitime.score} contre ${repli.score}`);
});

cas("le tri est total : score, puis focal, puis px", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const cs = [
    noeud([0, 0, 0], { focal: false }),
    noeud([mondePourPx(1200, 2), 0, 0], { focal: true }),
    noeud([mondePourPx(1200, 4), 0, 0], { focal: true }),
  ];
  const cl = classer(cs, cam, { x: W / 2, y: H / 2 }, 2);
  for (let i = 1; i < cl.length; i += 1) {
    const a = cl[i - 1];
    const b = cl[i];
    const ordonne = a.score > b.score
      || (a.score === b.score && Number(a.candidat.focal) >= Number(b.candidat.focal) && a.px <= b.px);
    assert.ok(ordonne, `désordre en ${i} : ${a.score}/${b.score}`);
  }
  assert.ok(cl[0].candidat.focal && cl[0].px <= cl[1].px);
});

cas("pureté : deux appels identiques rendent le même résultat", () => {
  const cam = camera([0, 0, 1200], [0, 0, 0]);
  const cs = [monde([0, 0, 0], 20, { focal: true }), noeud([mondePourPx(1200, 4), 0, 0])];
  const a = classer(cs, cam, { x: W / 2 + 1, y: H / 2 }, 2);
  const b = classer(cs, cam, { x: W / 2 + 1, y: H / 2 }, 2);
  assert.deepEqual(a.map((x) => [x.candidat.nom, x.score, x.px]), b.map((x) => [x.candidat.nom, x.score, x.px]));
});

// ---------------------------------------------------------------------------
// 12. RÉCAPITULATIF MESURÉ (les chiffres cités dans le module).
// ---------------------------------------------------------------------------
titre("récapitulatif mesuré");
const camRef = camera([0, 0, 1200], [0, 0, 0]);
const e20 = empreintePx(camRef, { x: 0, y: 0, z: 0 }, 20);
console.log(`  vue Cosmos de référence : ${W}×${H} px, fov ${FOV}, caméra à 1200 unités`);
console.log(`  un monde de rayon 20 à 1200 unités : empreinte ${e20.empreinte.toFixed(2)} px`);
console.log("  l'ancien seuil de 7 unités-monde, converti en pixels :");
for (const d of [100, 1200, 4000]) {
  console.log(`    à ${String(d).padStart(4)} unités : ${(7 * pxParUnite(d)).toFixed(2).padStart(6)} px`);
}
console.log(`  rayon de sélection d'un nœud (rayon monde 0) : ${REGLAGES_DEFAUT.plancherPx} px, à TOUTE distance`);
console.log("  formule : score = base · proxi · prec · vis · etage · face  (base = 100)");
console.log("            proxi = portee/(portee+distPx) · prec = 160/(160+empreinte)");
console.log("            vis   = 0,25 + 0,75·opacite · etage ∈ {8 / 1 / 0,2} · face ∈ {1 / 0,08}");

// ---------------------------------------------------------------------------
console.log(`\n${total - ko}/${total} cas passés${ko ? ` — ${ko} EN ÉCHEC` : ""}.`);
if (ko) process.exitCode = 1;
