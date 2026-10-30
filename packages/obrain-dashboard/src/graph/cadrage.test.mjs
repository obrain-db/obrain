/**
 * ── TEST DU MODULE PUR `cadrage.ts` — SANS FRAMEWORK ────────────────────────
 *
 * Compilation puis exécution (une seule ligne, depuis `packages/obrain-dashboard`) :
 *
 *   npx tsc src/graph/cadrage.ts --outDir /tmp/cad-test --module esnext \
 *     --target es2020 --moduleResolution bundler --strict \
 *     && node src/graph/cadrage.test.mjs
 *
 * Le test compile le module en ESM, puis le charge. Deux détails qui font
 * échouer une version naïve :
 *
 *   1. `/tmp/cad-test` n'a pas de `package.json`, donc Node lirait `cadrage.js`
 *      comme du CommonJS et refuserait ses `export`. On pose le marqueur
 *      `{"type":"module"}` AVANT d'importer — c'est le seul fichier que ce test
 *      écrit, et il l'écrit dans le répertoire de sortie de la compilation.
 *   2. `tsc` déduit sa racine commune du seul fichier d'entrée, mais elle peut
 *      changer selon la version : on essaie les emplacements possibles au lieu
 *      d'en supposer un.
 *
 * Aucune dépendance : `node:assert`, `node:fs`, `node:path`, `node:url`.
 */

import assert from "node:assert/strict";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

/* ── CHARGEMENT DU MODULE COMPILÉ ─────────────────────────────────────────── */

const SORTIE = process.env.CAD_TEST_OUT ?? "/tmp/cad-test";
mkdirSync(SORTIE, { recursive: true });
writeFileSync(join(SORTIE, "package.json"), '{ "type": "module" }\n');

const chemins = [
  join(SORTIE, "cadrage.js"),
  join(SORTIE, "graph", "cadrage.js"),
  join(SORTIE, "src", "graph", "cadrage.js"),
];
const trouve = chemins.find((p) => existsSync(p));
assert.ok(
  trouve,
  `module compilé introuvable — attendu l'un de : ${chemins.join(", ")}\n` +
    `as-tu lancé « npx tsc src/graph/cadrage.ts --outDir ${SORTIE} … » ?`,
);

const {
  butee,
  distanceCadrage,
  etape,
  ecartTransition,
  stabilise,
  cadragePour,
  demander,
  avancer,
} = await import(pathToFileURL(trouve).href);

/* ── HARNAIS MINIMAL, QUI MESURE AU LIEU D'ASSERTIONNER EN SILENCE ────────── */

let verts = 0;
let rouges = 0;
const mesures = new Map();

/** Enregistre une mesure nommée, pour l'afficher dans le bilan final. */
function mesure(nom, valeur) {
  mesures.set(nom, valeur);
  return valeur;
}

function verifier(nom, condition, detail = "") {
  if (condition) {
    verts++;
    console.log(`  \u001b[32m✓\u001b[0m ${nom}${detail ? ` — ${detail}` : ""}`);
  } else {
    rouges++;
    console.log(`  \u001b[31m✗\u001b[0m ${nom}${detail ? ` — ${detail}` : ""}`);
  }
}

function titre(t) {
  console.log(`\n\u001b[1m${t}\u001b[0m`);
}

const f2 = (x, n = 3) => Number(x).toFixed(n);
const dist = (a, b) => Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z);

/* ── LES CORPS DU TEST : les chiffres du défaut mesuré dans la vue ────────── */
/* Un monde de rayon 30 et un groupe de rayon 8,7 : le rapport 3,45× est
 * exactement celui qui faisait sauter la butée (2,4 → 2,2) et le centre
 * d'orbite. Le groupe n'est pas à l'origine : la transition doit aussi être
 * continue en POSITION, pas seulement en échelle. */

const MONDE_RAYON = 30;
const GROUPE_RAYON = 8.7;
const MONDE_POS = { x: 0, y: 0, z: 0 };
const GROUPE_POS = { x: 12, y: 3, z: -4 };
const DT_60 = 1 / 60;

const monde = () => cadragePour("monde", "monde-a", MONDE_POS, MONDE_RAYON);
const groupe = () => cadragePour("groupe", "groupe-b", GROUPE_POS, GROUPE_RAYON);
/** L'état de départ de l'intégrateur : rien n'a encore été posé. */
const vierge = () => ({ ...cadragePour("cosmos", "", { x: 0, y: 0, z: 0 }, 1), init: false });

/* ═══ 1. TRANSITION MONDE → GROUPE : CONTINUE, ET CONVERGENTE ══════════════ */

titre("1. Transition monde (rayon 30) → groupe (rayon 8,7)");

{
  const voulu = groupe();
  let actuel = monde();
  const ecarts = [ecartTransition(actuel, voulu)];
  const dPosTotale = dist(actuel.pos, voulu.pos);
  let maxSautEchelle = 0;
  let maxSautPos = 0;
  let t = 0;
  let frames = 0;

  // On avance par pas de 1/60 s jusqu'à ce que le module dise la transition
  // finie, avec un plafond de 10 s pour que le test se termine même en échec.
  while (!stabilise(actuel, voulu) && t < 10) {
    const avant = actuel;
    actuel = etape(avant, voulu, DT_60);
    t += DT_60;
    frames++;
    maxSautEchelle = Math.max(maxSautEchelle, Math.abs(actuel.rayon - avant.rayon) / avant.rayon);
    maxSautPos = Math.max(maxSautPos, dist(actuel.pos, avant.pos));
    ecarts.push(ecartTransition(actuel, voulu));
  }

  verifier(
    "aucun pas ne saute de plus de 5 % d'échelle",
    maxSautEchelle <= 0.05,
    `max mesuré ${f2(maxSautEchelle * 100, 2)} % / pas (k = ${f2(1 - Math.exp(-DT_60 * 2.2), 4)})`,
  );
  verifier(
    "la transition est finie en moins de 2 s",
    t < 2,
    `stabilisée en ${f2(t, 3)} s (${frames} images à 60 i/s)`,
  );
  verifier(
    "aucun pas ne saute de plus de 5 % de la course en position",
    maxSautPos <= dPosTotale * 0.05,
    `max ${f2(maxSautPos, 4)} u. pour une course de ${f2(dPosTotale, 3)} u.`,
  );

  // L'écart d'échelle doit DÉCROÎTRE STRICTEMENT (pas par paliers, pas de
  // rebond) : c'est la définition de « continu » côté mesure.
  let strict = true;
  for (let i = 1; i < ecarts.length; i++) if (!(ecarts[i] < ecarts[i - 1])) strict = false;
  verifier(
    "ecartTransition décroît strictement pendant toute la transition",
    strict,
    `${ecarts.length} points, de ${f2(ecarts[0], 4)} à ${f2(ecarts[ecarts.length - 1], 4)}`,
  );

  // Après convergence, on continue 3 s : l'écart doit être ~0, pas « petit ».
  let fin = actuel;
  for (let i = 0; i < 180; i++) fin = etape(fin, voulu, DT_60);
  const ecartFin = ecartTransition(fin, voulu);
  verifier("ecartTransition vaut ~0 à l'arrivée", ecartFin < 1e-3, `mesuré ${ecartFin.toExponential(2)}`);
  verifier(
    "le rayon a bien atteint celui du groupe",
    Math.abs(fin.rayon - GROUPE_RAYON) / GROUPE_RAYON < 1e-3,
    `${f2(fin.rayon, 4)} pour une cible de ${GROUPE_RAYON}`,
  );
  verifier(
    "le centre d'orbite a bien atteint celui du groupe",
    dist(fin.pos, GROUPE_POS) < 1e-3,
    `${f2(dist(fin.pos, GROUPE_POS), 6)} u. de la cible`,
  );
  verifier(
    "le frac a suivi le rayon (la loi de zoom ne saute pas)",
    Math.abs(fin.frac - 0.25) < 1e-3,
    `frac ${f2(0.08, 3)} → ${f2(fin.frac, 4)}`,
  );

  mesure("saut_echelle_max_pct", f2(maxSautEchelle * 100, 3));
  mesure("convergence_60ips_s", f2(t, 3));
}

/* ═══ 2. BUTÉE : DÉRIVÉE DU RAYON, PAS UNE CONSTANTE ═══════════════════════ */

titre("2. Butée — toujours dérivée du corps visé");

{
  const bMonde = mesure("butee_monde_30", butee(cadragePour("monde", "m", MONDE_POS, 30)));
  const bGroupe = mesure("butee_groupe_8_7", butee(cadragePour("groupe", "g", GROUPE_POS, GROUPE_RAYON)));

  verifier("la butée d'un groupe est plus petite que celle du monde", bGroupe < bMonde, `${f2(bGroupe)} < ${f2(bMonde)}`);
  verifier(
    "ce sont les valeurs mesurées du défaut (2,4 monde / ~2,2 groupe)",
    Math.abs(bMonde - 2.4) < 1e-9 && Math.abs(bGroupe - 2.175) < 1e-9,
    `${f2(bMonde, 4)} et ${f2(bGroupe, 4)}`,
  );

  // DÉRIVÉE DU RAYON : doubler le rayon double la butée, à genre constant.
  const b1 = butee(cadragePour("groupe", "g", GROUPE_POS, 8.7));
  const b2 = butee(cadragePour("groupe", "g", GROUPE_POS, 17.4));
  verifier("doubler le rayon double la butée (aucune constante absolue)", Math.abs(b2 / b1 - 2) < 1e-12, `${f2(b1, 4)} → ${f2(b2, 4)}`);

  // …et elle n'est PAS constante non plus d'un corps à l'autre : trois rayons
  // de monde différents donnent trois butées différentes.
  const b10 = butee(cadragePour("monde", "m", MONDE_POS, 10));
  const b20 = butee(cadragePour("monde", "m", MONDE_POS, 20));
  const b40 = butee(cadragePour("monde", "m", MONDE_POS, 40));
  verifier(
    "trois rayons de monde → trois butées différentes",
    b10 < b20 && b20 < b40,
    `${f2(b10, 3)} < ${f2(b20, 3)} < ${f2(b40, 3)} (×0,08)`,
  );
  verifier("la butée suit le genre via frac (0,08 monde / 0,25 groupe)", Math.abs(b40 - 3.2) < 1e-12 && Math.abs(b2 - 4.35) < 1e-12);

  // PLANCHERS : sous le plancher, la butée ne suit plus le rayon (mais elle ne
  // tombe jamais à zéro, sinon la caméra passerait dans son propre centre).
  const bMicro = butee(cadragePour("monde", "m", MONDE_POS, 0.5));
  const bNano = butee(cadragePour("groupe", "g", GROUPE_POS, 0.1));
  verifier("plancher monde : 1,2 pour un rayon de 0,5", Math.abs(bMicro - 1.2) < 1e-12, f2(bMicro, 4));
  verifier("plancher groupe : 0,4 pour un rayon de 0,1", Math.abs(bNano - 0.4) < 1e-12, f2(bNano, 4));

  // ROBUSTESSE : rayon nul, négatif, NaN, infini → plancher, jamais NaN.
  const hostile = [
    ["rayon 0", 0],
    ["rayon négatif", -5],
    ["rayon NaN", NaN],
    ["rayon +∞", Infinity],
  ].map(([nom, r]) => [nom, butee(cadragePour("groupe", "g", GROUPE_POS, r))]);
  verifier(
    "rayon nul/négatif/NaN/∞ → plancher, et jamais NaN",
    hostile.every(([, v]) => Number.isFinite(v) && v === 0.4),
    hostile.map(([n, v]) => `${n}=${f2(v, 2)}`).join(", "),
  );

  // Un cosmos se bute aussi sur son rayon : 650 × 0,08 = 52 unités, jamais une
  // distance absolue recopiée d'ailleurs.
  verifier("un cosmos (rayon 650) se bute à 52 u.", Math.abs(butee(cadragePour("cosmos", "c", MONDE_POS, 650)) - 52) < 1e-9);
}

/* ═══ 3. DISTANCE DE CADRAGE : UN MULTIPLE DU RAYON ════════════════════════ */

titre("3. Distance de cadrage (arrivée d'un vol)");

{
  const dMonde = distanceCadrage(cadragePour("monde", "m", MONDE_POS, 30));
  const dGroupe = distanceCadrage(cadragePour("groupe", "g", GROUPE_POS, GROUPE_RAYON));
  verifier("monde : 8,8 rayons (la visée mesurée de gotoWorld)", Math.abs(dMonde - 264) < 1e-9, `${f2(dMonde, 3)} u.`);
  verifier("groupe : 4,2 rayons (CADRAGE_POUPEE)", Math.abs(dGroupe - 36.54) < 1e-9, `${f2(dGroupe, 3)} u.`);
  verifier(
    "doubler le rayon double la distance (aucune distance absolue)",
    Math.abs(distanceCadrage(cadragePour("monde", "m", MONDE_POS, 60)) / dMonde - 2) < 1e-12,
  );
  verifier(
    "un facteur explicite remplace la loi du genre",
    Math.abs(distanceCadrage(cadragePour("groupe", "g", GROUPE_POS, 10), 3) - 30) < 1e-12,
  );
  verifier(
    "un facteur non fini retombe sur la loi du genre (jamais NaN)",
    distanceCadrage(groupe(), NaN) === dGroupe && distanceCadrage(groupe(), -2) === dGroupe,
  );
}

/* ═══ 4. LA DEMANDE : UNE INTENTION, PAS UNE ÉCRITURE ══════════════════════ */

titre("4. demander ne touche pas l'état vivant");

{
  const e0 = { actuel: monde(), voulu: monde(), demande: null };
  const e1 = demander(e0, groupe());

  verifier("actuel est rendu à l'identique (même référence)", Object.is(e1.actuel, e0.actuel));
  verifier("voulu est rendu à l'identique (même référence)", Object.is(e1.voulu, e0.voulu));
  verifier("actuel n'a pas bougé en valeur", e1.actuel.rayon === MONDE_RAYON && e1.actuel.genre === "monde" && e1.actuel.frac === 0.08);
  verifier("la demande, elle, est posée", e1.demande !== null && e1.demande.genre === "groupe" && e1.demande.rayon === GROUPE_RAYON);
  verifier("l'état d'entrée n'a pas été muté", e0.demande === null && e0.actuel.rayon === MONDE_RAYON);

  // La demande est COPIÉE : un appelant qui réutilise son objet ne peut pas
  // changer la cible en cours de route.
  const source = groupe();
  const e2 = demander(e0, source);
  source.rayon = 999;
  source.pos.x = 999;
  verifier("la demande est copiée (une source réutilisée ne la modifie pas)", e2.demande.rayon === GROUPE_RAYON && e2.demande.pos.x === GROUPE_POS.x);
}

/* ═══ 5. avancer : UN SEUL ÉCRIVAIN, QUI DIT QUI A ÉCRIT ══════════════════ */

titre("5. avancer — un seul écrivain, nommé");

{
  const e0 = { actuel: monde(), voulu: monde(), demande: null };

  // dt nul : rien n'est écrit, rien n'est consommé, et l'état est rendu TEL QUEL.
  const r0 = avancer(e0, 0);
  verifier("dt = 0 → état identique (même référence)", Object.is(r0.etat, e0) && r0.ecrivain === "immobile");
  const rNeg = avancer({ ...e0, demande: groupe() }, -1);
  verifier("dt < 0 → aucun changement, la demande reste en attente", Object.is(rNeg.etat.actuel, e0.actuel) && rNeg.etat.demande.rayon === GROUPE_RAYON);
  const rNan = avancer(e0, NaN);
  verifier("dt = NaN → aucun changement", Object.is(rNan.etat.actuel, e0.actuel) && rNan.ecrivain === "immobile");

  // Écart nul et pas de demande : immobile, et même référence (rien à écrire).
  const rImmobile = avancer(e0, DT_60);
  verifier("écart nul et pas de demande → immobile, sans copie", rImmobile.ecrivain === "immobile" && Object.is(rImmobile.etat, e0));

  // POURSUITE : la sélection (voulu) est rejointe.
  const ePoursuite = { actuel: monde(), voulu: groupe(), demande: null };
  const rP = avancer(ePoursuite, DT_60);
  verifier(
    "sans demande, c'est la sélection qui est rejointe → poursuite",
    rP.ecrivain === "poursuite" && rP.etat.actuel.rayon < MONDE_RAYON && rP.etat.actuel.rayon > GROUPE_RAYON,
    `rayon ${f2(rP.etat.actuel.rayon, 3)}`,
  );
  verifier("voulu est intact pendant la poursuite", Object.is(rP.etat.voulu, ePoursuite.voulu));

  // VOL : la demande prime sur la sélection. `voulu` désigne un AUTRE corps
  // (le monde d'un côté, la demande sur le groupe) : c'est la demande qui doit
  // gagner, et le rayon doit s'en rapprocher — pas celui du monde.
  const eVol = { actuel: monde(), voulu: monde(), demande: null };
  const eDemande = demander(eVol, groupe());
  const rV = avancer(eDemande, DT_60);
  verifier(
    "une demande en cours écrit en « vol » et prime sur voulu",
    rV.ecrivain === "vol" && rV.etat.actuel.rayon < MONDE_RAYON,
    `rayon ${f2(rV.etat.actuel.rayon, 3)} vers 8,7 (voulu = monde, rayon 30)`,
  );
  verifier("voulu reste la sélection tant que la demande tourne", rV.etat.voulu.rayon === MONDE_RAYON);
  verifier("la demande survit tant qu'elle n'est pas arrivée", rV.etat.demande !== null);

  // ARRIVÉE : la demande est consommée ET devient la référence. Sans cette
  // adoption, la caméra repartirait vers le monde : « j'avance et je repars en
  // arrière », le défaut documenté.
  let e = eDemande;
  let ecrivains = { vol: 0, poursuite: 0, immobile: 0 };
  let t = 0;
  let consommeeA = null;
  while (t < 10 && e.demande !== null) {
    const r = avancer(e, DT_60);
    e = r.etat;
    ecrivains[r.ecrivain]++;
    t += DT_60;
    if (e.demande === null) consommeeA = t;
  }
  verifier(
    "la demande est consommée à son arrivée et devient voulu",
    e.demande === null && e.voulu.genre === "groupe" && Math.abs(e.voulu.rayon - GROUPE_RAYON) < 1e-9,
    `consommée à ${f2(consommeeA, 3)} s`,
  );
  verifier("aucun pas de poursuite tant que le vol tourne", ecrivains.poursuite === 0, `vol×${ecrivains.vol}`);
  verifier(
    "PAS de retour en arrière après la demande",
    Math.abs(e.actuel.rayon - GROUPE_RAYON) / GROUPE_RAYON < 0.02,
    `rayon ${f2(e.actuel.rayon, 3)} (le monde ferait 30)`,
  );
  const rApres = avancer(e, DT_60);
  verifier(
    "une fois la demande consommée, plus personne n'écrit → immobile",
    rApres.ecrivain === "immobile" && Object.is(rApres.etat, e),
  );
  verifier(
    "l'autorité converge en moins de 2 s, comme la transition",
    consommeeA !== null && consommeeA < 2,
    `${f2(consommeeA, 3)} s`,
  );
}

/* ═══ 6. INVARIANCE AU FRAMERATE ═══════════════════════════════════════════ */

titre("6. Invariance au framerate");

{
  const voulu = groupe();
  const depart = monde();

  // Une suite de pas de durées très différentes (une image saccadée : 8 ms,
  // 42 ms, 11 ms, 83 ms…) doit mener au même état qu'UN pas de durée totale.
  // C'est vrai exactement, parce que le produit des (1 − k_i) vaut
  // exp(−TAUX × Σdt) : c'est la propriété qui fonde la loi exponentielle.
  const plan = [];
  for (let i = 0; i < 21; i++) plan.push(1 / 120, 1 / 24, 1 / 90, 1 / 12);
  const total = plan.reduce((s, d) => s + d, 0);

  let parPas = depart;
  for (const d of plan) parPas = etape(parPas, voulu, d);
  const enUnPas = etape(depart, voulu, total);

  const dRayon = Math.abs(parPas.rayon - enUnPas.rayon) / enUnPas.rayon;
  const dPos = dist(parPas.pos, enUnPas.pos);
  const dFrac = Math.abs(parPas.frac - enUnPas.frac);

  verifier(
    `rayon identique après ${plan.length} pas (Σ=${f2(total, 3)} s) et après 1 pas`,
    dRayon < 1e-9,
    `écart relatif ${dRayon.toExponential(2)} (arrondi flottant)`,
  );
  verifier("position identique (à l'arrondi flottant)", dPos < 1e-9, `écart ${dPos.toExponential(2)} u.`);
  verifier("frac identique", dFrac < 1e-9, `écart ${dFrac.toExponential(2)}`);

  // Contrôle croisé : c'est bien d'une invariance qu'il s'agit, pas d'une
  // convergence vers la cible (au bout de 3 s, les deux sont presque arrivés).
  const a60 = (() => {
    let a = depart;
    for (let i = 0; i < Math.round(total * 60); i++) a = etape(a, voulu, DT_60);
    return a;
  })();
  verifier(
    "60 i/s, 30 i/s et pas saccadés mènent au même cadrage",
    Math.abs(a60.rayon - enUnPas.rayon) / enUnPas.rayon < 1e-3,
    `60 i/s : ${f2(a60.rayon, 3)} vs 1 pas : ${f2(enUnPas.rayon, 3)} (troncature du nombre d'images)`,
  );

  // Et une invariance qui compte VRAIMENT à l'écran : la durée pour arriver ne
  // dépend pas du framerate.
  const duree = (dt) => {
    let a = depart;
    let t = 0;
    while (!stabilise(a, voulu) && t < 10) {
      a = etape(a, voulu, dt);
      t += dt;
    }
    return t;
  };
  const t60 = duree(1 / 60);
  const t30 = duree(1 / 30);
  const t15 = duree(1 / 15);
  verifier(
    "la durée de transition ne dépend pas du framerate",
    Math.abs(t60 - t30) < 0.05 && Math.abs(t60 - t15) < 0.1,
    `60 i/s ${f2(t60, 3)} s · 30 i/s ${f2(t30, 3)} s · 15 i/s ${f2(t15, 3)} s`,
  );
}

/* ═══ 7. PREMIÈRE POSE, IMMUTABILITÉ, SEUIL ════════════════════════════════ */

titre("7. Première pose, immutabilité, seuil");

{
  // init = faux : rien à quitter, donc pose DIRECTE — pas de transition depuis
  // un germe, qui serait le saut qu'on veut éviter, mais au premier frame.
  const g = vierge();
  const pose = etape(g, monde(), DT_60);
  verifier(
    "init faux → la première pose est directe (un seul pas suffit)",
    pose.init && pose.rayon === MONDE_RAYON && dist(pose.pos, MONDE_POS) === 0 && pose.frac === 0.08,
    `rayon ${f2(pose.rayon, 3)} dès le premier pas`,
  );
  verifier("la pose directe allume init", pose.init === true && g.init === false);

  // init faux côté VOULU : il n'y a pas de cible, donc rien à rejoindre.
  const sansCible = etape(monde(), vierge(), DT_60);
  verifier("init faux côté voulu → aucun mouvement (pas de transition vers le néant)", sansCible.rayon === MONDE_RAYON && Math.abs(ecartTransition(monde(), vierge())) === Infinity);

  // IMMUTABILITÉ : `etape` rend un NOUVEAU cadrage, sans toucher l'entrée.
  const a = monde();
  const b = groupe();
  const copieA = JSON.stringify(a);
  const copieB = JSON.stringify(b);
  const sortie = etape(a, b, DT_60);
  verifier("etape ne mute pas son entrée", JSON.stringify(a) === copieA && JSON.stringify(b) === copieB);
  verifier(
    "etape rend un NOUVEAU cadrage (pas de partage de pos)",
    sortie !== a && sortie.pos !== a.pos && sortie.genre === "groupe" && sortie.nom === "groupe-b",
  );
  verifier(
    "le genre et le nom suivent la cible dès le premier pas (étiquettes, pas grandeurs)",
    sortie.genre === "groupe" && sortie.frac < 0.25 && sortie.frac > 0.08,
    `frac ${f2(sortie.frac, 4)} (entre 0,08 et 0,25)`,
  );

  // SEUIL : un seuil explicite est respecté, un seuil absurde retombe sur la loi.
  const proche = { ...monde(), rayon: MONDE_RAYON * 1.02 };
  const loin = { ...monde(), rayon: MONDE_RAYON * 1.5 };
  const eProche = ecartTransition(monde(), proche);
  const eLoin = ecartTransition(monde(), loin);
  verifier("seuil explicite respecté", stabilise(monde(), proche, 0.03) === true && stabilise(monde(), proche, 0.01) === false, `écart ${f2(eProche, 4)}`);
  verifier(
    "seuil non fourni → celui du module (0,02)",
    stabilise(monde(), proche) === true && stabilise(monde(), loin) === false,
    `${f2(eProche, 4)} < 0,02 < ${f2(eLoin, 4)}`,
  );
  // Un seuil négatif ou NaN ne doit pas rendre TOUT « fini » (0, ce qui ferait
  // une boucle sans fin) ni tout « infini » : les deux écarts ci-dessus le
  // prouvent, ils retombent sur 0,02 dans les deux sens.
  verifier(
    "un seuil négatif ou NaN retombe sur le seuil du module (jamais une boucle sans fin)",
    stabilise(monde(), proche, -1) === true && stabilise(monde(), proche, NaN) === true &&
      stabilise(monde(), loin, -1) === false && stabilise(monde(), loin, NaN) === false,
  );

  // ROBUSTESSE : un rayon hostile ne doit jamais produire de NaN dans la
  // caméra — c'est la dernière ligne de défense du module.
  const hostile = etape(monde(), cadragePour("groupe", "g", GROUPE_POS, NaN), DT_60);
  const hostile2 = etape(cadragePour("monde", "m", MONDE_POS, 0), groupe(), 0.5);
  verifier(
    "rayon NaN/0 en entrée → aucun NaN en sortie",
    Number.isFinite(hostile.rayon) && Number.isFinite(hostile.pos.x) && hostile.rayon >= 1e-3 && Number.isFinite(hostile2.rayon),
  );
  verifier(
    "ecartTransition reste fini avec un rayon nul (ln(0) = −∞ évité)",
    Number.isFinite(ecartTransition(cadragePour("monde", "m", MONDE_POS, 0), groupe())),
  );
  const eHostile = avancer({ actuel: vierge(), voulu: monde(), demande: null }, DT_60);
  verifier("avancer sur un état vierge pose directement, sans NaN", eHostile.etat.actuel.rayon === MONDE_RAYON && Number.isFinite(eHostile.etat.actuel.pos.x));
}

/* ── BILAN ────────────────────────────────────────────────────────────────── */

console.log(`\n\u001b[1mBILAN\u001b[0m  ${verts} vérifications vertes, ${rouges} rouges`);
console.log("\nMESURES (les chiffres qui fondent le module) :");
for (const [nom, valeur] of mesures) console.log(`  · ${nom} = ${valeur}`);

if (rouges > 0) {
  console.log(`\n\u001b[31m${rouges} vérification(s) en échec\u001b[0m`);
  process.exitCode = 1;
} else {
  console.log("\n\u001b[32mTOUT PASSE\u001b[0m");
}
