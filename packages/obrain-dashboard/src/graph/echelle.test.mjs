/**
 * ── TEST DU MODULE PUR `echelle.ts` — COINS ET CAS LIMITES ──────────────────
 *
 * Compilation puis exécution (depuis `packages/obrain-dashboard`) :
 *
 *   npx tsc src/graph/echelle.ts --outDir /tmp/ech-test --module esnext \
 *     --target es2020 --moduleResolution bundler --strict \
 *     && node src/graph/echelle.test.mjs
 *
 * POURQUOI CE TEST EXISTE. La loi d'échelle a échoué le 2026-09-15 : les quatre
 * crans ne faisaient rien, et la cause était un cran qui VISAIT un monde pendant
 * que le palier en MESURAIT un autre. La loi vivait dans le composant, donc
 * n'était testable nulle part. Ici on tient l'invariant qui compte —
 * **un cran qui vise sa bande la retrouve** — et les coins : seuils exacts,
 * rayons dégénérés, distances impossibles, `t` hors bornes, monotonie.
 */
import assert from "node:assert/strict";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const SORTIE = process.env.ECH_TEST_OUT ?? "/tmp/ech-test";
mkdirSync(SORTIE, { recursive: true });
writeFileSync(join(SORTIE, "package.json"), '{ "type": "module" }\n');

const chemins = [
  join(SORTIE, "echelle.js"),
  join(SORTIE, "graph", "echelle.js"),
  join(SORTIE, "src", "graph", "echelle.js"),
];
const trouve = chemins.find((p) => existsSync(p));
assert.ok(
  trouve,
  `module compilé introuvable — attendu l'un de : ${chemins.join(", ")}\n` +
    `as-tu lancé « npx tsc src/graph/echelle.ts --outDir ${SORTIE} … » ?`,
);
const {
  BANDE_DEDANS, BANDE_MONDE, CRAN_T, PALIERS, PALIER_DEDANS_SIGNAUX, PALIER_SIGNAUX,
  distancePourPalier, distancePourT, nomPalier, palierDe, bandeDe, milieuBande, viserCran,
} = await import(pathToFileURL(trouve).href);

let ko = 0;
const cas = (nom, fn) => {
  try { fn(); console.log(`  ok   ${nom}`); }
  catch (e) { ko++; console.log(` FAIL  ${nom}\n         ${e.message.split("\n")[0]}`); }
};

console.log("échelle — coins et cas limites\n");

// ── 1. L'INVARIANT QUI COMPTE : UN CRAN QUI VISE SA BANDE LA RETROUVE ──────
// C'est exactement ce qui manquait le 2026-09-15 : viser et mesurer parlaient de
// deux mondes différents, donc un cran pouvait être parfaitement volé et jamais
// reconnu. Si cet aller-retour tient, viser et mesurer parlent de la même longueur.
cas("aller-retour : palierDe(distancePourPalier(n, R), R) === n (4 crans × 5 rayons)", () => {
  for (const R of [1, 25.99, 30, 56.4, 1500]) {
    for (const n of [0, 1, 2, 3]) {
      const d = distancePourPalier(n, R);
      assert.equal(palierDe(d, R), n, `n=${n} R=${R} d=${d}`);
    }
  }
});

cas("un cran plus profond est TOUJOURS plus près (aucun cran ne recule)", () => {
  for (const R of [1, 30, 1500]) {
    const ds = [0, 1, 2, 3].map((n) => distancePourPalier(n, R));
    for (let i = 1; i < ds.length; i++)
      assert.ok(ds[i] < ds[i - 1], `R=${R} : ${ds[i]} n'est pas plus près que ${ds[i - 1]}`);
  }
});

cas("les crans 1 et 2 tombent STRICTEMENT DANS leur bande (pas sur le bord)", () => {
  const R = 42.28;                                   // le rayon qui a échoué
  const dMonde = distancePourPalier(1, R);
  assert.ok(dMonde < distancePourT(BANDE_MONDE, R), "cran monde : au bord extérieur");
  assert.ok(dMonde > distancePourT(BANDE_DEDANS, R), "cran monde : au bord intérieur");
  const dGroupe = distancePourPalier(2, R);
  assert.ok(dGroupe < distancePourT(BANDE_DEDANS, R), "cran groupe : au bord extérieur");
  assert.ok(dGroupe > R * PALIER_DEDANS_SIGNAUX, "cran groupe : au bord intérieur");
});

// ── 2. LES SEUILS, EXACTEMENT — la convention doit être épinglée ───────────
// CONVENTION MESURÉE (et non supposée) : les comparaisons sont STRICTES
// (`d > seuil`), donc la distance qui vaut EXACTEMENT un seuil appartient à la
// bande INTÉRIEURE — celle du cran plus profond. C'est cohérent avec ce que les
// crans visent : ils tombent STRICTEMENT à l'intérieur de leur bande, donc le
// bord n'est jamais l'endroit où un cran se pose. Une convention non testée est
// une convention qui changera sans qu'on le sache : elle est donc testée ici,
// dans le sens où le code la tient.
cas("sur le seuil exact : on est DÉJÀ dans la bande intérieure (« monde »)", () => {
  const R = 30;
  const bord = distancePourT(BANDE_MONDE, R);
  assert.equal(palierDe(bord, R), 1, "le seuil exact appartient à la bande intérieure");
  assert.equal(palierDe(bord * 1.001, R), 0, "juste au-dessus : on est dehors");
  assert.equal(palierDe(bord * 0.999, R), 1);
});

cas("sur le seuil « dedans » : on est DÉJÀ dans le groupe", () => {
  const R = 30;
  const bord = distancePourT(BANDE_DEDANS, R);
  assert.equal(palierDe(bord, R), 2);
  assert.equal(palierDe(bord * 1.001, R), 1);
  assert.equal(palierDe(bord * 0.999, R), 2);
});

cas("sur `pres` : on est DÉJÀ dans l'espace des signaux", () => {
  const R = 30;
  assert.equal(palierDe(R * PALIER_DEDANS_SIGNAUX, R), 3);
  assert.equal(palierDe(R * PALIER_DEDANS_SIGNAUX * 1.001, R), 2);
  assert.equal(palierDe(R * PALIER_SIGNAUX, R), 3);
});

// ── 3. MONOTONIE : en s'approchant, le palier ne redescend jamais ──────────
cas("le palier ne décroît jamais quand on s'approche (balayage ×0,9 et −1)", () => {
  const R = 74.6;
  let precedent = 0;
  for (let d = R * 20; d > 0.01; d *= 0.9) {
    const p = palierDe(d, R);
    assert.ok(p >= precedent, `d=${d} : palier ${p} < ${precedent}`);
    precedent = p;
  }
  assert.equal(precedent, 3, "on doit finir au fond de l'échelle");
  precedent = 0;
  for (let d = R * 20; d > R * 0.01; d -= R * 0.05) {
    const p = palierDe(d, R);
    assert.ok(p >= precedent, `d=${d} : palier ${p} < ${precedent}`);
    precedent = p;
  }
});

// ── 4. RAYONS DÉGÉNÉRÉS — le garde doit être EXPLICITE, pas chanceux ───────
cas("rayon nul, négatif, NaN ou infini : on retombe sur 30, jamais sur NaN", () => {
  for (const mauvais of [0, -1, Number.NaN, Number.POSITIVE_INFINITY, Number.NEGATIVE_INFINITY]) {
    for (const n of [0, 1, 2, 3]) {
      const d = distancePourPalier(n, mauvais);
      assert.ok(Number.isFinite(d), `distancePourPalier(${n}, ${mauvais}) = ${d}`);
      const p = palierDe(d, mauvais);
      assert.equal(p, palierDe(d, 30), `incohérent avec R=30 pour R=${mauvais}`);
    }
    assert.ok(Number.isFinite(distancePourT(0.5, mauvais)));
  }
  // Le rayon DÉGÉNÉRÉ rend le même résultat que 30 : c'est le repli documenté.
  assert.equal(distancePourPalier(1, 0), distancePourPalier(1, 30));
  assert.equal(palierDe(100, Number.NaN), palierDe(100, 30));
});

// ── 5. DISTANCES IMPOSSIBLES ───────────────────────────────────────────────
cas("distance infinie → « espace » ; nulle ou négative → « signaux »", () => {
  const R = 30;
  assert.equal(palierDe(Number.POSITIVE_INFINITY, R), 0);
  assert.equal(palierDe(0, R), 3);
  assert.equal(palierDe(-5, R), 3);
});

cas("distance NaN → « espace » (on ne prétend pas être dedans sans savoir)", () => {
  assert.equal(palierDe(Number.NaN, 30), 0);
});

cas("`t` hors bornes est RAMENÉ, jamais extrapolé", () => {
  const R = 30;
  assert.equal(distancePourT(-5, R), distancePourT(0, R), "t négatif = tout au loin");
  assert.equal(distancePourT(5, R), distancePourT(1, R), "t > 1 sature");
  assert.equal(distancePourT(Number.NaN, R), distancePourT(0, R), "t NaN = borne basse");
  // Les bornes elles-mêmes : 14 R au plus loin, 0,25 R au plus près. La borne
  // HAUTE est exacte (une multiplication) ; la borne basse passe par exp/log,
  // donc elle est comparée en RELATIF — exiger l'égalité stricte sur un
  // logarithme serait tester le dernier bit du flottant, pas la loi.
  assert.equal(distancePourT(0, R), R * 14);
  const pres = distancePourT(1, R);
  assert.ok(Math.abs(pres - R * 0.25) < 1e-9 * R * 0.25, `borne basse : ${pres} ≠ ${R * 0.25}`);
});

cas("échelles extrêmes (R = 1e-6 et R = 1e9) : l'aller-retour tient encore", () => {
  for (const R of [1e-6, 1e9]) {
    for (const n of [0, 1, 2, 3]) {
      const d = distancePourPalier(n, R);
      assert.ok(Number.isFinite(d) && d > 0, `R=${R} n=${n} d=${d}`);
      assert.equal(palierDe(d, R), n, `R=${R} n=${n} d=${d}`);
    }
  }
});

cas("l'échelle est RELATIVE : doubler le rayon double toutes les distances", () => {
  for (const n of [0, 1, 2, 3]) {
    const a = distancePourPalier(n, 40), b = distancePourPalier(n, 80);
    assert.ok(Math.abs(b - 2 * a) < 1e-9 * Math.max(1, b), `cran ${n} : ${a} → ${b}`);
  }
});

// ── 6. LE VOCABULAIRE, écrit une fois ──────────────────────────────────────
cas("les quatre noms de palier sont ceux du produit, et un palier hors bornes ne casse rien", () => {
  assert.deepEqual([0, 1, 2, 3].map(nomPalier), [...PALIERS]);
  assert.equal(nomPalier(9), "espace");       // repli explicite, pas de plantage
  assert.equal(nomPalier(-1), "espace");
  assert.equal(PALIERS.length, 4);
});

cas("les constantes restent dans un ordre de descente cohérent", () => {
  assert.ok(CRAN_T.monde < CRAN_T.groupe, "le cran groupe doit viser plus loin dans la descente");
  assert.ok(BANDE_MONDE < BANDE_DEDANS, "les seuils de bande doivent être ordonnés");
  assert.ok(PALIER_SIGNAUX < PALIER_DEDANS_SIGNAUX, "l'espace des signaux est plus près que « dedans »");
  assert.ok(PALIER_DEDANS_SIGNAUX < 1, "« dedans » reste un seuil de distance, pas une distance absolue");
});

// ── 7. LA VISÉE D'UN CRAN EST GARANTIE DANS SA BANDE ─────────────────────
// C'est le correctif que ces coins ont rendu possible : mesuré, les crâns
// « groupe » et « signaux » visaient juste et la caméra se posait dans la bande
// « monde ». On ne se contente donc plus d'espérer `distancePourPalier` : on
// VÉRIFIE, et on corrige au milieu de la bande.
cas("la visée d'un cran tombe TOUJOURS dans sa bande (4 crans × 6 rayons)", () => {
  for (const R of [1e-6, 1, 24.2, 30, 56.4, 1500]) {
    for (const n of [0, 1, 2, 3]) {
      const d = viserCran(n, R);
      assert.ok(Number.isFinite(d) && d > 0, `n=${n} R=${R} d=${d}`);
      assert.equal(palierDe(d, R), n, `n=${n} R=${R} d=${d}`);
    }
  }
});

cas("chaque bande contient son milieu, et les bandes s'enchaînent sans trou ni recouvrement", () => {
  for (const R of [1, 24.2, 1500]) {
    for (const n of [0, 1, 2, 3]) {
      assert.equal(palierDe(milieuBande(n, R), R), n, `milieu de la bande ${n} (R=${R})`);
    }
    // Dehors → dedans : les bornes se touchent (convention : le seuil exact est
    // dans la bande INTÉRIEURE), donc aucun trou, aucun recouvrement.
    for (const n of [1, 2, 3]) {
      assert.equal(bandeDe(n, R).dehors, bandeDe(n - 1, R).dedans, `frontière ${n - 1}/${n}`);
    }
    assert.ok(bandeDe(0, R).dehors === Infinity, "l'espace n'a pas de borne extérieure");
    assert.equal(bandeDe(3, R).dedans, 0, "les signaux vont jusqu'au centre");
  }
});

cas("un cran plus PROFOND ne vise jamais plus loin que là où l'on est", () => {
  const R = 24.2;
  const dMonde = viserCran(1, R);
  // Depuis la bande « monde », viser « groupe » puis « signaux » : jamais plus loin.
  const dGroupe = viserCran(2, R, dMonde);
  const dSig = viserCran(3, R, dGroupe);
  assert.ok(dGroupe <= dMonde, `${dGroupe} > ${dMonde}`);
  assert.ok(dSig <= dGroupe, `${dSig} > ${dGroupe}`);
  assert.equal(palierDe(dGroupe, R), 2);
  assert.equal(palierDe(dSig, R), 3);
});

cas("remonter d'un cran reste possible (la garde ne bloque que la descente)", () => {
  const R = 24.2;
  const dSig = viserCran(3, R);
  const dMonde = viserCran(1, R, dSig);   // on REMONTE : la cible est plus loin, c'est voulu
  assert.ok(dMonde > dSig, "remonter doit éloigner la caméra");
  assert.equal(palierDe(dMonde, R), 1);
});

cas("visée sur un rayon dégénéré : jamais NaN, toujours dans la bande du repli", () => {
  for (const mauvais of [0, -1, Number.NaN, Number.POSITIVE_INFINITY]) {
    for (const n of [0, 1, 2, 3]) {
      const d = viserCran(n, mauvais);
      assert.ok(Number.isFinite(d) && d > 0, `n=${n} R=${mauvais} d=${d}`);
      assert.equal(palierDe(d, mauvais), n);
    }
  }
  assert.equal(viserCran(2, 0), viserCran(2, 30));
});

cas("une distance courante absurde ne casse pas la visée", () => {
  for (const d of [Number.NaN, Number.POSITIVE_INFINITY, -10, 0]) {
    const v = viserCran(2, 30, d);
    assert.equal(palierDe(v, 30), 2, `dActuelle=${d}`);
  }
});

console.log(ko === 0 ? "\n✓ la loi d'échelle tient sur tous les coins" : `\n✗ ${ko} cas en échec`);
process.exit(ko === 0 ? 0 : 1);
