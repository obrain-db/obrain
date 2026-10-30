/**
 * ── TEST DU MODULE PUR `densite.ts` — SANS FRAMEWORK ────────────────────────
 *
 * Compilation puis exécution (depuis `packages/obrain-dashboard`) :
 *
 *   npx tsc src/graph/densite.ts --outDir /tmp/dens-test --module esnext \
 *     --target es2020 --moduleResolution bundler --strict \
 *     && node src/graph/densite.test.mjs
 *
 * POURQUOI CE TEST EXISTE : ce module décide, à chaque geste, si l'on DENSIFIE
 * (montrer plus de matière au même niveau) ou si l'on APPROFONDIT (descendre
 * d'un cran). Se tromper d'un côté fait sauter un cran en laissant des groupes
 * invisibles (le défaut mesuré : 24 dessinés sur 200 servis) ; se tromper de
 * l'autre fait tourner en rond sur un niveau épuisé. Les deux se mesurent, donc
 * les deux se testent ici — sans navigateur.
 */
import assert from "node:assert/strict";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const SORTIE = process.env.DENS_TEST_OUT ?? "/tmp/dens-test";
mkdirSync(SORTIE, { recursive: true });
writeFileSync(join(SORTIE, "package.json"), '{ "type": "module" }\n');

const chemins = [
  join(SORTIE, "densite.js"),
  join(SORTIE, "graph", "densite.js"),
  join(SORTIE, "src", "graph", "densite.js"),
];
const trouve = chemins.find((p) => existsSync(p));
assert.ok(
  trouve,
  `module compilé introuvable — attendu l'un de : ${chemins.join(", ")}\n` +
    `as-tu lancé « npx tsc src/graph/densite.ts --outDir ${SORTIE} … » ?`,
);
const { decisionDensite, pageSuivante, densifiable, resteAMontrer, phraseDecision, libelleCourt, PAGE } =
  await import(pathToFileURL(trouve).href);

let ko = 0;
const cas = (nom, fn) => {
  try { fn(); console.log(`  ok   ${nom}`); }
  catch (e) { ko++; console.log(` FAIL  ${nom}\n         ${e.message.split("\n")[0]}`); }
};

console.log("densité — densifier ou approfondir\n");

// ── 1. LA LOI : densifier tant qu'il reste de la matière non montrée ────────
cas("200 reçus, 24 dessinés → DENSIFIER, sans appel, le niveau NE BOUGE PAS", () => {
  const d = decisionDensite({ total: 200, montres: 24 });
  assert.equal(d.geste, "densifier");
  assert.equal(d.dePlus, 24);            // 24 dessinés -> 48 : 24 de plus
  assert.equal(d.bougeLeNiveau, false);  // c'est la mesure qui sépare les deux gestes
  assert.equal(d.requiertAppel, false);  // la matière est DÉJÀ dans la réponse
});

cas("niveau épuisé (200 sur 200), sous-groupes dessous → APPROFONDIR", () => {
  const d = decisionDensite({ total: 200, montres: 200 });
  assert.equal(d.geste, "approfondir");
  assert.equal(d.bougeLeNiveau, true);
  assert.equal(d.dePlus, 0);
});

cas("niveau épuisé ET feuille → RIEN (on ne propose pas une descente vide)", () => {
  const d = decisionDensite({ total: 6, montres: 6, feuille: true });
  assert.equal(d.geste, "rien");
  assert.equal(d.bougeLeNiveau, false);
});

cas("plafond serveur atteint (200 reçus, 200 dessinés) → densifier PAR APPEL", () => {
  const d = decisionDensite({ total: 200, montres: 200, plafondAtteint: true });
  assert.equal(d.geste, "densifier");
  assert.equal(d.requiertAppel, true);   // ce n'est pas le même prix : on le DIT
  assert.equal(d.bougeLeNiveau, false);
});

// ── LE DÉFAUT MESURÉ AU-DELÀ DU PLAFOND (régression) ───────────────────────
// MESURÉ sur `runtime-dev` : le niveau `deepseek_harness` (rang 4) rend 200
// lignes avec `tronque: true`, dont un groupe au nom VIDE portant 8 438 nœuds —
// écarté par `tokenValide`. Le total FILTRÉ retombe donc sous 200 alors que le
// serveur a coupé. La règle 3 exigeait `total >= 200` : elle ne s'armait plus, et
// l'UI annonçait « approfondir » en abandonnant les pages 2..N. Ce cas est
// exactement celui-là, et il doit DENSIFIER — par appel, puisque la suite est
// chez le serveur.
cas("serveur tronqué, mais le FILTRE a réduit le total sous 200 → densifier PAR APPEL", () => {
  const d = decisionDensite({ total: 150, montres: 150, plafondAtteint: true });
  assert.equal(d.geste, "densifier");
  assert.equal(d.requiertAppel, true);
  assert.equal(d.bougeLeNiveau, false);
  assert.equal(densifiable({ total: 150, montres: 150, plafondAtteint: true }), true);
});

cas("sans `tronque`, un total sous 200 n'INVENTE pas de page suivante", () => {
  const d = decisionDensite({ total: 150, montres: 150 });
  assert.equal(d.geste, "approfondir");   // tout est montré, et le serveur n'a rien annoncé
});

cas("un groupe DÉSIGNÉ (« dedans ») l'emporte sur la densification", () => {
  const d = decisionDensite({ total: 200, montres: 24, intention: "dedans" });
  assert.equal(d.geste, "approfondir");
  assert.equal(d.bougeLeNiveau, true);
});

cas("intention « plus » sur un niveau épuisé : on le dit, et on approfondit", () => {
  const d = decisionDensite({ total: 12, montres: 12, intention: "plus" });
  assert.equal(d.geste, "approfondir");
  assert.match(phraseDecision({ total: 12, montres: 12 }, d), /approfondir/);
});

// ── 2. LA PAGE : bornée par ce qui existe, jamais au-delà ──────────────────
cas("la page suivante ne dépasse JAMAIS le total reçu", () => {
  assert.equal(pageSuivante(24, 200), 48);
  assert.equal(pageSuivante(190, 200), 200);
  assert.equal(pageSuivante(200, 200), 200);
  assert.equal(pageSuivante(0, 6), 6);        // un niveau de 6 : page unique
  assert.equal(pageSuivante(24, 0), 0);
});

cas("un état incohérent (plus dessiné que reçu) est RAMENÉ, pas cru", () => {
  assert.equal(resteAMontrer({ total: 10, montres: 99 }), 0);
  assert.equal(decisionDensite({ total: 10, montres: 99 }).geste, "approfondir");
});

cas("un niveau VIDE ne propose rien à densifier", () => {
  assert.equal(densifiable({ total: 0, montres: 0 }), false);
  assert.equal(decisionDensite({ total: 0, montres: 0, feuille: true }).geste, "rien");
});

// ── 3. LE PLAFOND : la phrase doit nommer la provenance de la suite ────────
cas("densifier SANS appel et densifier PAR APPEL ne disent pas la même chose", () => {
  const memoire = phraseDecision({ total: 200, montres: 24 });
  const appel = phraseDecision({ total: PAGE.plafondAppel, montres: PAGE.plafondAppel, plafondAtteint: true });
  assert.match(memoire, /révéler 24 de plus/);
  assert.match(appel, /serveur/);
  assert.notEqual(memoire, appel);
});

cas("le libellé court écrit le total dès qu'il dépasse ce qui est dessiné", () => {
  assert.match(libelleCourt({ total: 200, montres: 24 }), /24\/200/);
  assert.doesNotMatch(libelleCourt({ total: 6, montres: 6 }), /\//);
});

// ── 3 bis. LE NIVEAU DÉCOUPÉ SUR UN ÉCHANTILLON (le cas de la NAVIGATION) ──
// Mesuré : aucun niveau atteint par `Page ↓` n'a de page serveur pleine — les
// groupes viennent d'un échantillon du graphe (350 nœuds sur 52 979 réels, soit
// 0,09 %). Sans cette règle, « densifier » n'y était qu'un redessin et le « plus »
// n'existait pas là où l'on navigue.
cas("échantillon maigre (0,09 % du monde lu) → densifier l'ÉCHANTILLON, sans bouger", () => {
  const d = decisionDensite({ total: 5, montres: 5, source: "graphe", couverture: 0.0009 });
  assert.equal(d.geste, "densifier");
  assert.equal(d.bougeLeNiveau, false);
  assert.equal(d.requiertAppel, true);
  // AUCUN NOMBRE PROMIS : on ne sait pas combien de groupes un échantillon plus
  // large révélera, et l'annoncer serait inventer.
  assert.equal(d.dePlus, undefined);
  assert.match(phraseDecision({ total: 5, montres: 5, source: "graphe", couverture: 0.0009 }, d), /élargir/);
  assert.match(libelleCourt({ total: 5, montres: 5, source: "graphe", couverture: 0.0009 }, d), /élargir/);
});

cas("échantillon représentatif (90 % du monde lu) → le « plus » est un cran plus bas", () => {
  const d = decisionDensite({ total: 5, montres: 5, source: "graphe", couverture: 0.9 });
  assert.equal(d.geste, "approfondir");
  assert.equal(d.bougeLeNiveau, true);
});

cas("un niveau SERVEUR n'est jamais jugé sur la couverture d'un échantillon", () => {
  const d = decisionDensite({ total: 5, montres: 5, source: "serveur", couverture: 0.0009 });
  assert.equal(d.geste, "approfondir");
});

// ── 4. L'INVARIANT QUI SÉPARE LES DEUX GESTES ─────────────────────────────
// C'est l'exigence du contrôle d'acceptation : densifier ne change QUE le
// nombre dessiné ; approfondir change le préfixe et le rang. Aucune entrée ne
// doit produire un « densifier » qui bouge le niveau, ni l'inverse.
cas("AUCUN cas ne densifie en bougeant le niveau, ni n'approfondit sans le bouger", () => {
  const cas_tests = [];
  for (const total of [0, 1, 6, 24, 25, 200, 500])
    for (const montres of [0, 1, 24, 25, 200, 500])
      for (const feuille of [false, true])
        for (const plafondAtteint of [false, true])
          for (const intention of [null, "plus", "dedans"])
            for (const source of [undefined, "serveur", "graphe"])
              for (const couverture of [undefined, 0.0009, 0.9])
                cas_tests.push({ total, montres, feuille, plafondAtteint, intention, source, couverture });
  for (const e of cas_tests) {
    const d = decisionDensite(e);
    assert.equal(d.bougeLeNiveau, d.geste === "approfondir", JSON.stringify(e));
    // `dePlus` n'est absent QUE pour l'échantillon (règle 3 bis) : ailleurs, un
    // densifier qui n'annonce aucun nombre est un densifier qui n'ajoute rien.
    if (d.geste === "densifier")
      assert.ok(d.dePlus === undefined ? e.source === "graphe" && e.couverture < 0.5 : d.dePlus > 0,
        "un densifier sans compte : " + JSON.stringify(e));
    if (d.geste === "rien") assert.equal(d.dePlus, 0);
    if (d.geste !== "densifier") assert.equal(d.dePlus >= 0, true, JSON.stringify(e));
  }
});

cas("densifier ne propose JAMAIS plus que ce qui reste à montrer", () => {
  for (let total = 0; total <= 240; total += 7)
    for (const montres of [0, 10, 24, 48, 199]) {
      const d = decisionDensite({ total, montres });
      if (d.geste === "densifier" && !d.requiertAppel && typeof d.dePlus === "number")
        assert.ok(d.dePlus <= Math.max(0, total - Math.min(montres, total)) + PAGE.pas, "trop: " + total + "/" + montres);
    }
});

// ── 5. LES COINS DE LA LOI DE DÉCISION ────────────────────────────────────
// Chaque cas ici est un bord RÉEL : un seuil qu'on croyait évident, un état
// impossible qui arrive quand même (mesure manquante, total négatif, NaN), ou
// une cohérence entre deux autorités qui pourraient diverger sans qu'on le voie.

cas("la couverture se juge au SEUIL EXACT : 0,5 n'est PAS maigre (strictement <)", () => {
  const geste = (c) => decisionDensite({ total: 5, montres: 5, source: "graphe", couverture: c }).geste;
  assert.equal(geste(0.5), "approfondir", "pile au seuil : l'échantillon est réputé représentatif");
  assert.equal(geste(0.499999), "densifier");
  assert.equal(geste(0.500001), "approfondir");
});

cas("une couverture manquante ou impossible ne fait PAS densifier (on ne juge pas sans unité)", () => {
  for (const c of [undefined, Number.NaN, Number.POSITIVE_INFINITY]) {
    const d = decisionDensite({ total: 5, montres: 5, source: "graphe", couverture: c });
    assert.equal(d.geste, "approfondir", `couverture=${c}`);
  }
  // Et sans `source`, la couverture n'a aucun sens : elle ne doit rien déclencher.
  assert.equal(decisionDensite({ total: 5, montres: 5, couverture: 0.0001 }).geste, "approfondir");
});

cas("un état IMPOSSIBLE est ramené, jamais cru (négatifs, NaN, flottants)", () => {
  for (const e of [
    { total: -5, montres: -3 }, { total: Number.NaN, montres: Number.NaN },
    { total: 3.7, montres: 1.2 }, { total: 10, montres: 1e9 }, { total: 1e9, montres: 0 },
  ]) {
    const d = decisionDensite(e);
    assert.ok(d.dePlus === undefined || d.dePlus >= 0, JSON.stringify(e));
    assert.ok(Number.isFinite(d.dePlus ?? 0), JSON.stringify(e));
    assert.ok(!Number.isNaN(d.bougeLeNiveau), JSON.stringify(e));
  }
  assert.equal(pageSuivante(-4, 10), 10);
  // `montres` illisible = « rien de dessiné » : la page suivante est entière,
  // bornée par le total (jamais NaN, qui remonterait jusqu au dessin).
  assert.equal(pageSuivante(Number.NaN, 10), 10);
  assert.equal(pageSuivante(0, Number.NaN, Number.NaN), 0);
});

cas("le pas de page est BORNÉ, jamais nul ni négatif ni NaN", () => {
  assert.equal(pageSuivante(0, 100, 1), 1);
  assert.equal(pageSuivante(0, 100, 0), 1, "un pas nul ferait une page qui ne progresse pas");
  assert.equal(pageSuivante(0, 100, -5), 1);
  // Un pas ILLISIBLE retombe sur le pas PAR DÉFAUT (24) : lire « 1 » serait
  // inventer une page d un seul élément, ce que personne n a demandé.
  assert.equal(pageSuivante(0, 100, Number.NaN), 24);
  assert.equal(pageSuivante(0, 100, 1e9), 100, "un pas énorme reste borné par le total");
});

cas("un groupe DÉSIGNÉ sur une FEUILLE : la règle 1 l'emporte, et c'est VOULU", () => {
  // La règle 1 passe avant la règle 4 : même sur une feuille, une direction
  // désignée est une direction. C'est la surface qui n'offre pas le geste
  // (`!interieur.feuille`) — la loi, elle, répond ce qu'on lui demande.
  const d = decisionDensite({ total: 6, montres: 6, feuille: true, intention: "dedans" });
  assert.equal(d.geste, "approfondir");
  assert.equal(d.bougeLeNiveau, true);
});

cas("page serveur PLEINE sur un niveau d'échantillon : c'est le SERVEUR qui tranche (règle 3)", () => {
  // Cas impossible par construction (la surface éteint `plafondAtteint` quand la
  // source est le graphe), mais la loi ne doit pas dépendre d'un état qu'elle ne
  // contrôle pas : si le serveur a coupé, la suite est chez lui.
  const d = decisionDensite({ total: 8, montres: 8, source: "graphe", couverture: 0.9, plafondAtteint: true });
  assert.equal(d.geste, "densifier");
  assert.equal(d.requiertAppel, true);
  assert.equal(d.dePlus, PAGE.pas, "là, un nombre est PROMIS : c'est une page comptable");
});

cas("les deux autorités publiées ne se contredisent jamais (densifiable ⇔ geste)", () => {
  // `densifiable` et `decisionDensite` répondent à la même question par deux
  // chemins. Deux chemins qui divergent donneraient une puce offerte que le geste
  // refuse — le défaut déjà mesuré à l'Atlas. On les tient donc côte à côte.
  for (const total of [0, 1, 24, 200])
    for (const montres of [0, 1, 24, 200])
      for (const feuille of [false, true])
        for (const plafondAtteint of [false, true])
          for (const source of [undefined, "serveur", "graphe"])
            for (const couverture of [undefined, 0.0009, 0.9]) {
              const e = { total, montres, feuille, plafondAtteint, source, couverture };
              const veutDensifier = decisionDensite(e).geste === "densifier";
              assert.equal(densifiable(e), veutDensifier, `divergence : ${JSON.stringify(e)}`);
            }
});

cas("aucune phrase affichée ne contient « undefined » ni « NaN »", () => {
  // Une phrase est ce que l'utilisateur LIT : y laisser fuiter un trou de donnée
  // est exactement ce que ce module refuse. Balayage complet des états.
  for (const total of [0, 1, 24, 200, 500])
    for (const montres of [0, 1, 24, 200, 500])
      for (const feuille of [false, true])
        for (const plafondAtteint of [false, true])
          for (const intention of [null, "plus", "dedans"])
            for (const source of [undefined, "serveur", "graphe"])
              for (const couverture of [undefined, 0.0009, 0.9]) {
                const e = { total, montres, feuille, plafondAtteint, intention, source, couverture };
                for (const texte of [phraseDecision(e), libelleCourt(e)]) {
                  assert.ok(!/undefined|NaN/.test(texte), `${texte} ← ${JSON.stringify(e)}`);
                  assert.ok(texte.length > 0, "une phrase vide ne dit rien");
                }
              }
});

cas("densifier en mémoire n'annonce JAMAIS plus qu'il ne reste", () => {
  for (let total = 0; total <= 120; total += 3)
    for (const montres of [0, 1, 23, 24, 25, 119, 120]) {
      const d = decisionDensite({ total, montres });
      if (d.geste !== "densifier" || d.requiertAppel) continue;
      const reste = Math.max(0, total - Math.min(montres, total));
      assert.ok(d.dePlus <= reste, `annonce ${d.dePlus} pour ${reste} restant(s)`);
      assert.ok(d.dePlus <= PAGE.pas, "une page ne dépasse pas un pas");
    }
});

// ── 6. LES TROIS BORDS QUE LA SECTION 5 NE COUVRAIT PAS ENCORE ─────────────
// Cette section a été écrite en relisant la 5 : la plupart des coins que
// l'intuition propose (NaN, négatifs, flottants, seuil exact, pas nul, pas
// illisible) y étaient DÉJÀ — le pas entier et son repli sur la valeur par
// défaut viennent même d'y être corrigés après qu'un cas de coin a montré que
// `Math.max(1, NaN)` vaut NaN. On n'ajoute donc ici que ce qui MANQUAIT : un
// test redondant ne protège rien et fait croire à une couverture qui n'a pas
// grandi.
cas("un total NÉGATIF ne rend pas une page négative", () => {
  // Une réponse incohérente (total négatif) ne doit ni rendre une page négative,
  // ni faire croire qu'il reste de la matière.
  assert.equal(pageSuivante(0, -10), 0);
  assert.equal(pageSuivante(-10, -10), 0);
  assert.equal(resteAMontrer({ total: -10, montres: -10 }), 0);
});

cas("un `montres` ILLISIBLE vaut « rien de dessiné », donc tout reste à montrer", () => {
  // Cas réel : le nombre dessiné vient d'un rendu précédent qui n'a pas abouti.
  // Le lire comme « tout est montré » ferait annoncer « approfondir » alors que
  // rien n'est à l'écran.
  assert.equal(resteAMontrer({ total: 10, montres: Number.NaN }), 10);
  assert.equal(resteAMontrer({ total: 10, montres: undefined }), 10);
  assert.equal(decisionDensite({ total: 10, montres: Number.NaN }).geste, "densifier");
});

cas("une FEUILLE qui porte de la matière non montrée DENSIFIE quand même", () => {
  // Ordre DÉCLARÉ des règles : « il reste quelque chose à dessiner » (règle 2)
  // passe avant « c'est une feuille » (règle 4). Une feuille dit qu'il n'y a rien
  // en DESSOUS, pas qu'il n'y a rien à MONTRER ici.
  const d = decisionDensite({ total: 200, montres: 24, feuille: true });
  assert.equal(d.geste, "densifier");
  assert.equal(d.bougeLeNiveau, false);
  assert.equal(d.dePlus, 24);
});

console.log(ko === 0 ? "\n✓ la loi tient sur tous les cas" : `\n✗ ${ko} cas en échec`);
process.exit(ko === 0 ? 0 : 1);
