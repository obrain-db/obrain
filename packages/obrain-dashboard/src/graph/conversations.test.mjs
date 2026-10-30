/**
 * ── TEST DU MODULE PUR `conversations.ts` — SANS FRAMEWORK ──────────────────
 *
 * Compilation puis exécution (depuis `packages/obrain-dashboard`) :
 *
 *   npx tsc src/graph/conversations.ts --outDir /tmp/conv-test --module esnext \
 *     --target es2020 --moduleResolution bundler --strict \
 *     && node src/graph/conversations.test.mjs
 *
 * POURQUOI CE TEST EXISTE : ce module décide ce que l'utilisateur VOIT de ses
 * échanges — leur nom, leur ordre, leur rangement, et ce qui survit à une
 * suppression. Deux défauts y seraient invisibles à la relecture et coûteux pour
 * l'utilisateur : une conversation qui change de nom toute seule (on ne la
 * reconnaît plus) et une suppression qui laisse la liste VIDE (l'écran n'a plus
 * où écrire). Les deux se mesurent, donc les deux se testent ici.
 */
import assert from "node:assert/strict";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const SORTIE = process.env.CONV_TEST_OUT ?? "/tmp/conv-test";
mkdirSync(SORTIE, { recursive: true });
writeFileSync(join(SORTIE, "package.json"), '{ "type": "module" }\n');
const chemins = [
  join(SORTIE, "conversations.js"),
  join(SORTIE, "graph", "conversations.js"),
  join(SORTIE, "src", "graph", "conversations.js"),
];
const trouve = chemins.find((p) => existsSync(p));
assert.ok(trouve, `module compilé introuvable — attendu l'un de : ${chemins.join(", ")}\n` +
  `as-tu lancé « npx tsc src/graph/conversations.ts --outDir ${SORTIE} … » ?`);
const C = await import(pathToFileURL(trouve).href);

let ko = 0;
const cas = (nom, fn) => {
  try { fn(); console.log(`  ok   ${nom}`); }
  catch (e) { ko++; console.log(` FAIL  ${nom}\n         ${e.message.split("\n")[0]}`); }
};
const tour = (role, texte, extra = {}) => ({ role, texte, ...extra });
const jeu = () => [
  tour("user", "  Combien de fonctions\n dans le noyau ?  "),
  tour("assistant", "…"),
];

console.log("conversations — structurer les échanges\n");

// ── 1. LE TITRE CITE, IL N'INVENTE PAS ─────────────────────────────────────
cas("le titre cite la PREMIÈRE question, espaces normalisés", () => {
  assert.equal(C.titreAuto(jeu()), "Combien de fonctions dans le noyau ?");
});

cas("un fil sans question n'a pas de titre (on ne fabrique pas un faux nom)", () => {
  assert.equal(C.titreAuto([tour("assistant", "bonjour")]), "");
  assert.equal(C.titreAuto([]), "");
  assert.equal(C.titreAuto([tour("user", "   ")]), "");
});

cas("un titre trop long est coupé SUR UN MOT, jamais au milieu", () => {
  const long = "mot ".repeat(30).trim();
  const t = C.titreAuto([tour("user", long)]);
  assert.ok(t.endsWith("…"), t);
  assert.ok(t.length <= 43, `longueur ${t.length} : ${t}`);
  assert.ok(!/ m…$/.test(t), "coupé dans un mot : " + t);
});

cas("« sans titre » est reconnu comme un titre qui n'apprend rien", () => {
  assert.equal(C.titreVide(""), true);
  assert.equal(C.titreVide("   "), true);
  assert.equal(C.titreVide("sans titre"), true);
  assert.equal(C.titreVide("Sans Titre"), true);
  assert.equal(C.titreVide("Noyau"), false);
});

// ── 2. L'ORDRE EST UNE DÉCISION, PAS UN HASARD ─────────────────────────────
cas("l'ordre : épinglées d'abord, puis la plus récemment touchée", () => {
  const a = { ...C.nouvelleConversation(1000, "a"), maj: 1000, epingle: false };
  const b = { ...C.nouvelleConversation(2000, "b"), maj: 2000, epingle: false };
  const c = { ...C.nouvelleConversation(1500, "c"), maj: 1500, epingle: true };
  assert.deepEqual(C.trier([a, b, c]).map((x) => x.id), ["c", "b", "a"]);
});

cas("à date égale, l'ID départage : deux rendus ne peuvent pas diverger", () => {
  const x = { ...C.nouvelleConversation(10, "zz"), maj: 10 };
  const y = { ...C.nouvelleConversation(10, "aa"), maj: 10 };
  assert.deepEqual(C.trier([x, y]).map((k) => k.id), ["aa", "zz"]);
  assert.deepEqual(C.trier([y, x]).map((k) => k.id), ["aa", "zz"]);
});

// ── 3. RANGER : DES DOSSIERS RÉELS, COMPTÉS ───────────────────────────────
cas("les dossiers sont ceux qui EXISTENT, comptés, « non rangées » en dernier", () => {
  let e = C.etatNeuf(1);
  e = C.ajouter(e, 2); e = C.ajouter(e, 3);
  const [i1, i2, i3] = e.liste.map((c) => c.id);
  e = C.ranger(e, i1, "noyau");
  e = C.ranger(e, i2, "noyau");
  e = C.ranger(e, i3, "  ");
  assert.deepEqual(C.dossiers(e), [{ nom: "noyau", n: 2 }, { nom: "", n: 1 }]);
});

cas("changer de dossier ne change PAS l'identité ni les tours", () => {
  let e = C.etatNeuf(1);
  e = C.majCourante(e, jeu(), 5);
  const id = C.courante(e).id;
  const apres = C.ranger(e, id, "noyau");
  const c = apres.liste.find((x) => x.id === id);
  assert.equal(c.dossier, "noyau");
  assert.equal(c.tours.length, 2);
  assert.equal(c.id, id);
});

// ── 4. LE TITRE SE POSE UNE FOIS (sinon on ne reconnaît plus rien) ────────
cas("le titre automatique se pose à la PREMIÈRE question et ne bouge plus", () => {
  let e = C.etatNeuf(1);
  e = C.majCourante(e, jeu(), 5);
  const t1 = C.courante(e).titre;
  assert.equal(t1, "Combien de fonctions dans le noyau ?");
  // Nouvelle question dans la MÊME conversation : le nom ne doit pas danser.
  e = C.majCourante(e, [...jeu(), tour("user", "et en C++ ?")], 9);
  assert.equal(C.courante(e).titre, t1);
});

cas("un nom DONNÉ par l'utilisateur n'est jamais écrasé par l'automatique", () => {
  let e = C.etatNeuf(1);
  const id = C.courante(e).id;
  e = C.renommer(e, id, "Noyau — comptage");
  e = C.majCourante(e, jeu(), 9);
  assert.equal(C.courante(e).titre, "Noyau — comptant".replace("comptant", "comptage"));
});

// ── 5. SUPPRIMER NE LAISSE JAMAIS LA LISTE VIDE ───────────────────────────
cas("supprimer la conversation ouverte en ouvre une autre (jamais de vide)", () => {
  let e = C.etatNeuf(1);
  e = C.ajouter(e, 2); e = C.ajouter(e, 3);
  const ids = e.liste.map((c) => c.id);
  e = C.supprimer(e, ids[0], 4);
  assert.equal(e.liste.length, 2);
  assert.ok(e.liste.some((c) => c.id === e.courante), "la courante doit exister");
  assert.ok(C.courante(e), "courante() doit toujours rendre une conversation");
});

cas("supprimer la DERNIÈRE rend une conversation NEUVE et VIDE (pas la supprimée)", () => {
  let e = C.etatNeuf(1);
  const id = C.courante(e).id;
  e = C.majCourante(e, jeu(), 5);
  e = C.supprimer(e, id, 7);
  assert.equal(e.liste.length, 1);
  assert.notEqual(e.liste[0].id, id);
  assert.deepEqual(e.liste[0].tours, []);
  assert.equal(e.liste[0].titre, "");
});

cas("choisir un identifiant inconnu ne casse rien (aucun changement)", () => {
  const e = C.etatNeuf(1);
  assert.deepEqual(C.choisir(e, "inexistant"), e);
});

// ── 6. MIGRATION DE L'ANCIEN FORMAT ──────────────────────────────────────
cas("l'ancienne conversation est ADOPTÉE, titrée, et ses tours sont repris", () => {
  const ancien = [tour("user", "Question d'avant", { encours: true }), tour("assistant", "…", { reprise: true })];
  const e = C.migrer(ancien, 100);
  assert.equal(e.liste.length, 1);
  assert.equal(e.liste[0].tours.length, 2);
  assert.equal(e.liste[0].titre, "Question d'avant");
  assert.equal(e.liste[0].tours[0].encours, false, "un tour « en cours » restauré ferait tourner une attente éternelle");
  assert.equal(e.liste[0].tours[0].reprise, false);
});

cas("rien à migrer → `null` : la migration ne fabrique pas une conversation", () => {
  assert.equal(C.migrer([], 1), null);
  assert.equal(C.migrer(null, 1), null);
  assert.equal(C.migrer(undefined, 1), null);
});

// ── 7. CE QUI EST ÉCRIT EST BORNÉ, ET SANS ATTENTE ────────────────────────
cas("ce qui est persisté exclut les tours EN COURS et borne à `maxTours`", () => {
  let e = C.etatNeuf(1);
  const tours = [...Array.from({ length: 50 }, (_, i) => tour("user", `q${i}`)), tour("assistant", "en vol", { encours: true })];
  e = C.majCourante(e, tours, 5);
  const p = C.aPersister(e, 40);
  const gardes = p.liste[0].tours;
  assert.equal(gardes.length, 40);
  assert.ok(!gardes.some((x) => x.encours), "un tour en cours ne doit jamais être écrit");
  assert.equal(gardes[gardes.length - 1].texte, "q49");
});

// ── 8. L'INVARIANT, SUR UNE SUITE DE GESTES ──────────────────────────────
cas("après n'importe quelle suite de gestes, la liste vit et la courante existe", () => {
  let e = C.etatNeuf(1);
  const gestes = [
    (s) => C.ajouter(s, 2), (s) => C.ajouter(s, 3),
    (s) => C.renommer(s, s.liste[0].id, "un"), (s) => C.ranger(s, s.liste[1].id, "noyau"),
    (s) => C.epingler(s, s.liste[2].id), (s) => C.choisir(s, s.liste[1].id),
    (s) => C.majCourante(s, jeu(), 9), (s) => C.supprimer(s, s.liste[0].id, 10),
    (s) => C.supprimer(s, s.courante, 11), (s) => C.epingler(s, s.courante),
  ];
  for (const g of gestes) {
    e = g(e);
    assert.ok(e.liste.length >= 1, "la liste ne doit jamais être vide");
    assert.ok(e.liste.some((c) => c.id === e.courante), "la courante doit exister dans la liste");
    assert.ok(C.courante(e), "courante() doit rendre une conversation");
    for (const c of e.liste) {
      assert.equal(typeof c.dossier, "string");
      assert.equal(typeof c.epingle, "boolean");
      assert.ok(Number.isFinite(c.maj) && Number.isFinite(c.cree));
      assert.ok(Array.isArray(c.tours));
    }
  }
});

console.log(ko === 0 ? "\n✓ les conversations tiennent sur tous les cas" : `\n✗ ${ko} cas en échec`);
process.exit(ko === 0 ? 0 : 1);
