/**
 * ── TEST DU MODULE PUR `segments.ts` — SANS FRAMEWORK ───────────────────────
 *
 * Compilation puis exécution (depuis `packages/obrain-dashboard`) :
 *
 *   npx tsc src/graph/segments.ts --outDir /tmp/seg-test --module esnext \
 *     --target es2020 --moduleResolution bundler --strict \
 *     && node src/graph/segments.test.mjs
 *
 * POURQUOI CE TEST EXISTE : le découpage décide de ce que l'utilisateur voit —
 * un appel d'outil posé au mauvais endroit coupe un tableau en deux, ou fait
 * avaler le reste de la réponse par un bloc de code non refermé. Ces deux cas
 * sont exactement ce qu'on teste ici, avec la géométrie réelle du texte.
 */
import assert from "node:assert/strict";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const SORTIE = process.env.SEG_TEST_OUT ?? "/tmp/seg-test";
mkdirSync(SORTIE, { recursive: true });
writeFileSync(join(SORTIE, "package.json"), '{ "type": "module" }\n');

const chemins = [
  join(SORTIE, "segments.js"),
  join(SORTIE, "graph", "segments.js"),
  join(SORTIE, "src", "graph", "segments.js"),
];
const trouve = chemins.find((p) => existsSync(p));
assert.ok(
  trouve,
  `module compilé introuvable — attendu l'un de : ${chemins.join(", ")}\n` +
    `as-tu lancé « npx tsc src/graph/segments.ts --outDir ${SORTIE} … » ?`,
);
const { segmentsDeReponse } = await import(pathToFileURL(trouve).href);

const appel = (pos, nom = "obrain_query") => ({
  nom, arguments: "MATCH (n) RETURN count(n)", lignes: 1, ms: 3, monde: "", pos,
});
let ko = 0;
const cas = (nom, fn) => {
  try { fn(); console.log(`  ok   ${nom}`); }
  catch (e) { ko++; console.log(` FAIL  ${nom}\n         ${e.message.split("\n")[0]}`); }
};
const formes = (segs) => segs.map((s) => ("outil" in s ? `O(${s.outil.nom})` : `T(${s.texte.length})`)).join(" ");

/* 1. sans outil : un seul morceau, intact */
cas("sans appel d'outil, le texte reste d'un seul tenant", () => {
  const segs = segmentsDeReponse("Bonjour\n\nle monde", []);
  assert.equal(segs.length, 1);
  assert.equal(segs[0].texte, "Bonjour\n\nle monde");
});

/* 2. un appel au milieu d'un paragraphe : ramené à la ligne vide qui précède */
cas("un appel au milieu d'un paragraphe ne coupe pas le paragraphe", () => {
  const texte = "Premier paragraphe.\n\nDeuxième paragraphe, plus long.";
  const pos = texte.indexOf("Deuxième") + 8;   // au milieu du second
  const segs = segmentsDeReponse(texte, [appel(pos)]);
  assert.deepEqual(formes(segs), `T(21) O(obrain_query) T(${texte.length - 21})`);
  assert.equal(segs[0].texte, "Premier paragraphe.\n\n");
  assert.ok(segs[2].texte.startsWith("Deuxième"), "le second paragraphe est entier");
});

/* 3. un appel DANS un bloc de code ouvert : repoussé après la clôture */
cas("un appel dans un bloc de code est repoussé après sa clôture", () => {
  const texte = "Voici :\n\n```rust\nlet x = 1;\n```\n\nEt la suite.";
  const pos = texte.indexOf("let x") + 3;      // dans le bloc ouvert
  const segs = segmentsDeReponse(texte, [appel(pos)]);
  const iOutil = segs.findIndex((s) => "outil" in s);
  const avant = segs.slice(0, iOutil).map((s) => s.texte).join("");
  const nbBalises = (avant.match(/```/g) || []).length;
  assert.equal(nbBalises % 2, 0, `l'appel ne doit pas tomber DANS un bloc de code ouvert (vu ${nbBalises} balises avant)`);
  assert.ok(segs[iOutil + 1].texte.includes("Et la suite."));
});

/* 4. deux appels : l'ordre est celui des positions */
cas("deux appels gardent l'ordre de leurs positions", () => {
  const texte = "A.\n\nB.\n\nC.";
  const segs = segmentsDeReponse(texte, [appel(texte.indexOf("C") + 1, "deuxieme"), appel(texte.indexOf("B") + 1, "premier")]);
  const noms = segs.filter((s) => "outil" in s).map((s) => s.outil.nom);
  assert.deepEqual(noms, ["premier", "deuxieme"]);
});

/* 5. appels sans position (historique antérieur) : à la fin, dans l'ordre */
cas("un appel sans position est posé à la fin", () => {
  const segs = segmentsDeReponse("Une réponse.", [{ nom: "ancien", arguments: "", lignes: 0, ms: 0, monde: "" }]);
  assert.deepEqual(formes(segs), "T(12) O(ancien)");
});

/* 6. texte vide (l'appel précède toute réponse) : aucun morceau vide */
cas("un appel en tête de réponse ne produit aucun morceau vide", () => {
  const segs = segmentsDeReponse("", [appel(0)]);
  assert.deepEqual(formes(segs), "O(obrain_query)");
});

/* 7. position au-delà du texte : on ne dépasse pas */
cas("une position au-delà du texte pose l'appel APRÈS le texte, jamais avant", () => {
  const segs = segmentsDeReponse("court", [appel(9999)]);
  assert.deepEqual(formes(segs), "T(5) O(obrain_query)");
});
cas("sans ligne vide, l'appel ne passe pas AVANT son propre texte", () => {
  const segs = segmentsDeReponse("un seul paragraphe assez long", [appel(10)]);
  assert.deepEqual(formes(segs), "T(29) O(obrain_query)");
});

/* 8. appel AVANT tout texte : il vient en tête */
cas("un appel en position 0 vient avant le texte", () => {
  const segs = segmentsDeReponse("La réponse.\n\nSuite.", [appel(0)]);
  assert.deepEqual(formes(segs), "O(obrain_query) T(19)");
});

console.log(ko === 0
  ? "\nOK : le découpage place chaque appel sans jamais couper un bloc Markdown ni un bloc de code"
  : `\nÉCHEC : ${ko} cas non conforme(s)`);
process.exit(ko === 0 ? 0 : 1);
