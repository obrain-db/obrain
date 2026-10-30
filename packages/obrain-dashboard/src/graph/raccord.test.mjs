/**
 * TESTS DU RACCORD DE REPRISE — Node pur, aucun framework, aucune dépendance.
 *
 * Les deux cas testés ici sont CEUX QUI ONT ÉTÉ MESURÉS dans la journée sur le
 * chat du port 8432, à la virgule près (voir `raccord.ts`) : une continuation
 * exacte d'un côté, une réponse RÉGÉNÉRÉE de l'autre — parce que le modèle fait
 * l'un ou l'autre, et que le raccord doit être juste dans les deux cas.
 *
 * Exécution (celle des autres tests du dossier) :
 *   cd packages/obrain-dashboard
 *   npx tsc src/graph/raccord.ts --outDir /tmp/raccord-test --module esnext \
 *     --target es2020 --moduleResolution bundler --strict
 *   node src/graph/raccord.test.mjs
 */

import assert from "node:assert/strict";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const SORTE = process.env.RACCORD_TEST_OUT ?? "/tmp/raccord-test";
mkdirSync(SORTE, { recursive: true });
writeFileSync(join(SORTE, "package.json"), '{ "type": "module" }\n');

const PISTES = [
  process.env.RACCORD_JS,
  join(SORTE, "graph", "raccord.js"),
  join(SORTE, "raccord.js"),
].filter(Boolean);
const chemin = PISTES.find((p) => existsSync(p));
if (!chemin) {
  console.log(`SKIP — module compilé introuvable (essayé : ${PISTES.join(", ")})`);
  console.log("       compilez d'abord : npx tsc src/graph/raccord.ts --outDir " + SORTE);
  process.exit(0);
}
const { sansRecouvrement, estRegeneration, raccorder } = await import(pathToFileURL(chemin).href);

let faits = 0;
const verifier = (condition, quoi) => {
  faits++;
  console.log(`${condition ? "  ok  " : " FAIL "} ${quoi}`);
  assert.ok(condition, quoi);
};

// ── 1. LE CAS MESURÉ : le modèle CONTINUE là où il s'est arrêté ─────────────
// « …16, 17, 1 » puis « 8, 19, 20 » : le « 8 » termine le 18. Rien ne doit être
// retiré — le « 8 » n'appartient pas à la fin affichée.
{
  const deja = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 1";
  const suite = "8, 19, 20";
  const r = raccorder(deja, suite);
  verifier(r.remplace === false, "continuation : on AJOUTE, on ne remplace pas");
  verifier(r.ajout === suite, `continuation : la suite est intacte (${JSON.stringify(r.ajout)})`);
  verifier(deja + r.ajout === deja + suite, "continuation : le texte final est la concaténation exacte");
}

// ── 2. LE SECOND CAS MESURÉ : le modèle RECOMMENCE la réponse ───────────────
// « 1, 2, … 20 » après une coupure en plein compte : coller les deux donnerait
// « …16, 17, 1, 1, 2, 3… » — le doublon qui se lit comme un défaut du logiciel.
{
  const deja = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 1";
  const suite = "1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20.\n\nAucune réponse inventée.";
  const r = raccorder(deja, suite);
  verifier(r.remplace === true, "régénération : la suite REMPLACE le texte affiché");
  verifier(r.ajout === suite, "régénération : c'est la réponse entière du modèle qui s'affiche");
  verifier(!(deja + r.ajout).includes("1, 1,"), "régénération : aucun doublon ne subsiste");
}

// ── 3. LE RECOUVREMENT COURT : le modèle répète ses derniers mots ───────────
// Il finit « …la cause est donc le » et reprend « le noyau d'agrégats ». Ici le
// doublon est réel et borné : on le retire.
{
  const deja = "Après mesure, la cause du coût est le noyau d'agrégats";
  const suite = "noyau d'agrégats qui relit chaque nœud à chaque requête.";
  const r = raccorder(deja, suite);
  verifier(r.remplace === false, "recouvrement : on ajoute (la suite n'est pas une réponse entière)");
  verifier(r.ajout === " qui relit chaque nœud à chaque requête.", `recouvrement retiré (${JSON.stringify(r.ajout)})`);
  verifier(!(deja + r.ajout).includes("agrégatsnoyau"), "recouvrement : le doublon de fin de phrase est retiré");
}

// ── 4. LA PROPRIÉTÉ INVERSE : on ne retire JAMAIS du texte neuf ─────────────
// Un mot court qui se répète (« 1 », « la ») ne doit pas être avalé : couper trop
// ferait disparaître ce que le modèle vient d'écrire, un défaut pire que le doublon.
{
  const deja = "Le compte est 1";
  const suite = "1, 2, 3 — je continue.";
  // Recouvrement de 1 caractère, sous le seuil : rien n'est retiré.
  verifier(sansRecouvrement(deja, suite) === suite, "un recouvrement court ne retire RIEN");
  const phrase = "et la";
  const suite2 = "la même erreur se répète";
  verifier(sansRecouvrement(phrase, suite2) === "la même erreur se répète",
    "un article répété n'est pas un doublon");
}

// ── 5. LA CASSE ET LES ESPACES NE DÉCIDENT PAS ──────────────────────────────
// Le modèle ne réécrit pas son texte à l'octet près (espaces, majuscules) : une
// régénération doit être reconnue malgré ces écarts, sinon le doublon s'affiche.
{
  const deja = "Voici la reponse :\n1. la premiere cause";
  const suite = "voici la reponse : 1. la premiere cause, puis les autres.";
  verifier(estRegeneration(deja, suite) === true, "régénération reconnue malgré espaces et casse");
  const autre = "Une toute autre réponse, qui ne reprend pas le début.";
  verifier(estRegeneration(deja, autre) === false, "une réponse DIFFÉRENTE n'est pas prise pour une régénération");
}

console.log(`── ${faits} vérifications — raccord de reprise`);
