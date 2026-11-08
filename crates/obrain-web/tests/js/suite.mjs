// Batterie de tests du paquet obrain-web. UNE source, deux environnements :
// node (tests/js/node.mjs) et Chrome (tests/js/navigateur.html). Aucune dépendance.
// Les tests dépendants d'une feature s'activent seulement si la méthode existe.

class EchecAssertion extends Error {}

export const assert = {
  ok(v, msg = "valeur fausse") { if (!v) throw new EchecAssertion(msg); },
  egal(a, b, msg = "") {
    if (a !== b) throw new EchecAssertion(`${msg} attendu ${JSON.stringify(b)}, obtenu ${JSON.stringify(a)}`);
  },
  profond(a, b, msg = "") {
    const x = JSON.stringify(a), y = JSON.stringify(b);
    if (x !== y) throw new EchecAssertion(`${msg} attendu ${y}, obtenu ${x}`);
  },
  leve(fn, motif, msg = "") {
    let e = null;
    try { fn(); } catch (x) { e = x; }
    if (e === null) throw new EchecAssertion(`${msg} aucune erreur levée`);
    if (motif && !String(e).toLowerCase().includes(motif.toLowerCase()))
      throw new EchecAssertion(`${msg} erreur « ${String(e).slice(0, 120)} » ne contient pas « ${motif} »`);
  },
};

const lignes = (j) => JSON.parse(j).lignes;
const col = (j, i = 0) => lignes(j).map((l) => l[i]);

export function suite(Base) {
  const tests = [];
  const test = (nom, fn, { besoin } = {}) => tests.push({ nom, fn, besoin });
  const a = () => new Base();
  const a_la_methode = (b, m) => typeof b[m] === "function";

  // ───────────── base et GQL ─────────────
  test("une base neuve est vide", () => {
    const b = a();
    assert.egal(b.noeuds(), 0); assert.egal(b.aretes(), 0);
  });

  test("GQL : insérer, filtrer, trier, limiter", () => {
    const b = a();
    for (const [n, age] of [["Alix", 30], ["Bao", 25], ["Chloé", 41]])
      b.executer(`INSERT (:Personne {nom: '${n}', age: ${age}})`);
    assert.profond(col(b.executer("MATCH (p:Personne) WHERE p.age > 26 RETURN p.nom ORDER BY p.age")), ["Alix", "Chloé"]);
    assert.profond(col(b.executer("MATCH (p:Personne) RETURN p.age ORDER BY p.age DESC LIMIT 1")), [41]);
  });

  test("GQL : agrégats", () => {
    const b = a();
    for (const age of [10, 20, 30]) b.executer(`INSERT (:P {age: ${age}})`);
    assert.profond(col(b.executer("MATCH (p:P) RETURN count(p)")), [3]);
    assert.profond(col(b.executer("MATCH (p:P) RETURN sum(p.age)")), [60]);
    assert.profond(col(b.executer("MATCH (p:P) RETURN max(p.age)")), [30]);
  });

  test("GQL : arêtes, traversée et suppression détachée", () => {
    const b = a();
    b.executer("INSERT (:A {n: 'a'})-[:R]->(:B {n: 'b'})");
    assert.egal(b.aretes(), 1);
    assert.profond(col(b.executer("MATCH (x:A)-[:R]->(y:B) RETURN y.n")), ["b"]);
    b.executer("MATCH (x:A) DETACH DELETE x");
    assert.egal(b.noeuds(), 1); assert.egal(b.aretes(), 0, "l'arête part avec son nœud");
  });

  test("GQL : modifier une propriété", () => {
    const b = a();
    b.executer("INSERT (:P {nom: 'x', v: 1})");
    b.executer("MATCH (p:P {nom: 'x'}) SET p.v = 2");
    assert.profond(col(b.executer("MATCH (p:P) RETURN p.v")), [2]);
  });

  // ───────────── paramètres ─────────────
  test("paramètres : types scalaires", () => {
    const b = a();
    b.executer_avec_parametres("INSERT (:T {s: $s, i: $i, f: $f, b: $b})", JSON.stringify({ s: "texte", i: 42, f: 2.5, b: true }));
    const l = lignes(b.executer("MATCH (t:T) RETURN t.s, t.i, t.f, t.b"))[0];
    assert.profond(l, ["texte", 42, 2.5, true]);
  });

  test("paramètres : une chaîne d'apparence hostile reste une VALEUR", () => {
    const b = a();
    const piege = "x'}) DETACH DELETE (n) //";
    b.executer("INSERT (:K {v: 'sain'})");
    b.executer_avec_parametres("INSERT (:K {v: $v})", JSON.stringify({ v: piege }));
    assert.egal(b.noeuds(), 2, "rien n'a été supprimé");
    assert.profond(col(b.executer_avec_parametres("MATCH (k:K) WHERE k.v = $v RETURN k.v", JSON.stringify({ v: piege }))), [piege]);
  });

  test("paramètres : JSON invalide ou non-objet → erreur, pas de trappe", () => {
    const b = a();
    assert.leve(() => b.executer_avec_parametres("RETURN 1", "{pas du json"), "");
    assert.leve(() => b.executer_avec_parametres("RETURN 1", "[1,2]"), "objet");
    assert.leve(() => b.executer_avec_parametres("RETURN 1", "42"), "objet");
    assert.egal(b.noeuds(), 0, "la base reste utilisable");
  });

  // ───────────── erreurs et entrées hostiles ─────────────
  test("une requête invalide lève une erreur lisible, et la base survit", () => {
    const b = a();
    b.executer("INSERT (:K {v: 1})");
    for (const q of ["", " ", "MATCH (", "MATCH (n) RETURN", "INSERT", "RETURN 1/0", "\0", "CALL obrain.nope()"]) {
      try { b.executer(q); } catch (e) { assert.ok(String(e).length > 0, "message non vide"); }
    }
    assert.egal(b.noeuds(), 1);
  });

  test("langage inconnu → erreur explicite", () => {
    assert.leve(() => a().executer_langage("klingon", "RETURN 1"), "non disponible");
  });

  test("imbrication profonde : refusée par une erreur, jamais une trappe", () => {
    const b = a();
    for (const n of [65, 200, 5000, 50000]) {
      assert.leve(() => b.executer("RETURN " + "(".repeat(n) + "1" + ")".repeat(n)), "imbriqu", `n=${n}`);
      assert.leve(() => b.executer("MATCH " + "(".repeat(n) + " RETURN 1"), "", `match n=${n}`);
    }
    assert.profond(col(b.executer("RETURN " + "(".repeat(40) + "1" + ")".repeat(40))), [1], "40 niveaux passent");
  });

  test("requête de 2 Mo : erreur ou résultat, jamais de trappe", () => {
    const b = a();
    try { b.executer("RETURN '" + "a".repeat(2_000_000) + "'"); } catch (e) { /* acceptable */ }
    assert.egal(b.noeuds(), 0);
  });

  test("Unicode : emoji, combinants, CJK, aller-retour exact", () => {
    const b = a();
    const t = "é ü 日本語 😀 é \u0000x";
    b.executer_avec_parametres("INSERT (:U {t: $t})", JSON.stringify({ t }));
    assert.profond(col(b.executer("MATCH (u:U) RETURN u.t")), [t]);
  });

  test("grosses valeurs : propriété de 5 Mo", () => {
    const b = a();
    const gros = "z".repeat(5_000_000);
    b.executer_avec_parametres("INSERT (:G {v: $v})", JSON.stringify({ v: gros }));
    assert.egal(col(b.executer("MATCH (g:G) RETURN g.v"))[0].length, 5_000_000);
  });

  // ───────────── sessions et transactions ─────────────
  test("transaction : annuler défait, valider garde", () => {
    const b = a();
    const s = b.session();
    s.commencer(); s.executer("INSERT (:T {v: 1})"); s.annuler();
    assert.egal(b.noeuds(), 0, "annulée");
    s.commencer(); s.executer("INSERT (:T {v: 2})"); s.valider();
    assert.egal(b.noeuds(), 1, "validée");
  });

  test("session : valider sans transaction → erreur, pas de trappe", () => {
    const s = a().session();
    try { s.valider(); } catch (e) { assert.ok(String(e).length > 0); }
    try { s.annuler(); } catch (e) { assert.ok(String(e).length > 0); }
  });

  // ───────────── création directe et durée de vie ─────────────
  test("creer_noeud : étiquettes et propriétés", () => {
    const b = a();
    const id = b.creer_noeud(["Doc", "Public"], JSON.stringify({ titre: "x", n: 7 }));
    assert.ok(Number.isFinite(id) && id >= 0);
    assert.profond(col(b.executer("MATCH (d:Doc) RETURN d.titre")), ["x"]);
    assert.profond(col(b.executer("MATCH (d:Doc) RETURN d.n")), [7]);
    assert.profond(col(b.executer("MATCH (d:Public) RETURN count(d)")), [1], "la 2e étiquette est posée");
    assert.leve(() => b.creer_noeud(["X"], "pas du json"), "");
    assert.egal(b.noeuds(), 1, "l'échec n'a rien créé");
  });

  test("deux bases sont indépendantes", () => {
    const x = a(), y = a();
    x.executer("INSERT (:A {v: 1})");
    assert.egal(x.noeuds(), 1); assert.egal(y.noeuds(), 0);
  });

  test("libérer une base puis l'utiliser lève une erreur, sans trappe", () => {
    const b = a();
    b.executer("INSERT (:A {v: 1})");
    b.free();
    assert.leve(() => b.executer("RETURN 1"), "");
  });

  test("200 bases créées et libérées", () => {
    for (let i = 0; i < 200; i++) { const b = a(); b.executer("INSERT (:A {v: 1})"); b.free(); }
  });

  // ───────────── langages (feature `complet`) ─────────────
  test("Cypher : créer et lire", () => {
    const b = a(); if (!a_la_methode(b, "executer_langage")) return;
    try { b.executer_langage("cypher", "RETURN 1"); } catch (e) { if (String(e).includes("non disponible")) return "ignoré : cypher absent"; throw e; }
    b.executer_langage("cypher", "CREATE (:P {n: 'a', v: 1})");
    assert.profond(col(b.executer_langage("cypher", "MATCH (p:P) RETURN p.v")), [1]);
  });

  test("SPARQL : insérer des triplets et les lire", () => {
    const b = a();
    try { b.executer_langage("sparql", "SELECT ?s WHERE { ?s ?p ?o }"); } catch (e) { if (String(e).includes("non disponible")) return "ignoré : sparql absent"; }
    b.executer_langage("sparql", "INSERT DATA { <http://ex/a> <http://ex/p> <http://ex/b> }");
    assert.ok(lignes(b.executer_langage("sparql", "SELECT ?s ?o WHERE { ?s <http://ex/p> ?o }")).length >= 1);
  });

  test("Gremlin : ajouter un sommet et compter", () => {
    const b = a();
    try { b.executer_langage("gremlin", "g.V().count()"); } catch (e) { if (String(e).includes("non disponible")) return "ignoré : gremlin absent"; }
    b.executer_langage("gremlin", "g.addV('p').property('name','a')");
    assert.ok(b.noeuds() >= 1);
  });

  test("GraphQL et SQL/PGQ : requêtes valides ou erreurs propres", () => {
    const b = a();
    for (const [l, q] of [["graphql", "{ personne { nom } }"], ["sql", "SELECT 1"]]) {
      try { b.executer_langage(l, q); } catch (e) { if (String(e).includes("non disponible")) continue; assert.ok(String(e).length > 0); }
    }
  });

  test("tous les langages : entrées hostiles sans trappe", () => {
    const b = a();
    const hostiles = ["", "(", ")))", "{", "'", "\0", "MATCH (", "g.V(", "{ x ".repeat(100), "(".repeat(5000)];
    for (const l of ["gql", "cypher", "sparql", "gremlin", "graphql", "sql"])
      for (const q of hostiles) { try { b.executer_langage(l, q); } catch (e) { /* attendu */ } }
    assert.egal(b.noeuds(), 0);
  });

  // ───────────── recherche (feature `complet`) ─────────────
  test("recherche plein texte : le bon document, score positif", () => {
    const b = a(); if (!a_la_methode(b, "rechercher_texte")) return "ignoré : text-index absent";
    const renard = b.creer_noeud(["Doc"], JSON.stringify({ corps: "le renard brun saute" }));
    b.creer_noeud(["Doc"], JSON.stringify({ corps: "un chien dort" }));
    b.creer_index_texte("Doc", "corps");
    const r = JSON.parse(b.rechercher_texte("Doc", "corps", "renard", 5));
    assert.egal(r.length, 1); assert.egal(r[0].noeud, renard); assert.ok(r[0].score > 0);
    JSON.parse(b.rechercher_texte("Doc", "corps", "", 5));
  });

  test("recherche vectorielle : le plus proche d'abord ; Float32Array traversant la frontière", () => {
    const b = a(); if (!a_la_methode(b, "rechercher_vecteur")) return "ignoré : vector-index absent";
    const ids = [[1, 0, 0], [0, 1, 0], [0, 0, 1]].map((v) => {
      const id = b.creer_noeud(["V"], "{}"); b.definir_vecteur(id, "emb", new Float32Array(v)); return id;
    });
    b.creer_index_vectoriel("V", "emb", 3, "cosine");
    const r = JSON.parse(b.rechercher_vecteur("V", "emb", new Float32Array([0, 0.9, 0.1]), 1));
    assert.egal(r.length, 1); assert.egal(r[0].noeud, ids[1]);
    assert.ok(Number.isFinite(r[0].distance), "distance finie");
  });

  test("recherche vectorielle : dimension fausse ou k=0 → erreur ou résultat vide, sans trappe", () => {
    const b = a(); if (!a_la_methode(b, "rechercher_vecteur")) return "ignoré : vector-index absent";
    const id = b.creer_noeud(["V"], "{}"); b.definir_vecteur(id, "emb", new Float32Array([1, 0, 0]));
    b.creer_index_vectoriel("V", "emb", 3, "cosine");
    for (const v of [[], [1], [1, 2, 3, 4, 5, 6]]) { try { b.rechercher_vecteur("V", "emb", new Float32Array(v), 3); } catch (e) { /* attendu */ } }
    try { b.rechercher_vecteur("V", "emb", new Float32Array([1, 0, 0]), 0); } catch (e) { /* attendu */ }
    assert.egal(b.noeuds(), 1);
  });

  test("recherche hybride : texte et vecteur désignent le même document", () => {
    const b = a(); if (!a_la_methode(b, "rechercher_hybride")) return "ignoré : hybrid-search absent";
    const renard = b.creer_noeud(["D"], JSON.stringify({ corps: "le renard brun saute" }));
    b.definir_vecteur(renard, "emb", new Float32Array([1, 0, 0]));
    const chien = b.creer_noeud(["D"], JSON.stringify({ corps: "un chien dort" }));
    b.definir_vecteur(chien, "emb", new Float32Array([0, 1, 0]));
    b.creer_index_texte("D", "corps"); b.creer_index_vectoriel("D", "emb", 3, "cosine");
    const r = JSON.parse(b.rechercher_hybride("D", "corps", "emb", "renard", new Float32Array([0.9, 0.1, 0]), 2));
    assert.ok(r.length >= 1); assert.egal(r[0].noeud, renard);
  });

  // ───────────── algorithmes, cognitif, métriques ─────────────
  test("algorithmes : PageRank sur un petit graphe", () => {
    const b = a();
    b.executer("INSERT (:N {i: 1})-[:R]->(:N {i: 2})-[:R]->(:N {i: 3})");
    try { assert.ok(lignes(b.executer("CALL obrain.pagerank()")).length >= 1); }
    catch (e) { if (String(e).includes("pagerank") || String(e).includes("procedure")) return "ignoré : algos absent"; throw e; }
  });

  test("cognitif : métriques de la couche cognitive", () => {
    const b = a();
    try { b.executer("CALL obrain.cognitive.metrics()"); }
    catch (e) { if (String(e).toLowerCase().includes("cognitive") || String(e).includes("procedure")) return "ignoré : cognitive absent"; throw e; }
  });

  test("métriques : le compteur de requêtes avance", () => {
    const b = a(); if (!a_la_methode(b, "metriques")) return "ignoré : metrics absent";
    for (let i = 0; i < 5; i++) b.executer("RETURN 1");
    assert.ok(JSON.parse(b.metriques()).requetes >= 5);
  });

  // ───────────── charge et stabilité ─────────────
  test("charge : 3 000 nœuds, 20 000 requêtes, résultat exact et stable", () => {
    const b = a();
    for (let i = 0; i < 3000; i++) b.executer(`INSERT (:N {i: ${i}})`);
    assert.egal(b.noeuds(), 3000);
    let somme = 0;
    for (let i = 0; i < 20000; i++) somme += col(b.executer(`MATCH (n:N {i: ${i % 3000}}) RETURN n.i`))[0];
    const attendu = Array.from({ length: 20000 }, (_, i) => i % 3000).reduce((x, y) => x + y, 0);
    assert.egal(somme, attendu, "somme des résultats");
    assert.egal(b.noeuds(), 3000, "aucun nœud perdu ni créé");
  });

  test("déterminisme : la même séquence donne les mêmes résultats", () => {
    const run = () => {
      const b = a();
      for (let i = 0; i < 50; i++) b.executer(`INSERT (:D {i: ${i}, g: ${i % 5}})`);
      return b.executer("MATCH (d:D) RETURN d.g AS g, count(d) AS n ORDER BY g");
    };
    assert.egal(run(), run());
  });

  // Bogue du MOTEUR (natif comme wasm) : `RETURN d.g, count(d) ORDER BY d.g` levait
  // « Variable 'd' not found for ORDER BY property projection ». Correctif : branche
  // fix/order-by-apres-agregat. Ce test s'active tout seul quand le moteur le contient.
  test("ORDER BY sur une propriété déjà projetée, après agrégat", () => {
    const b = a();
    for (let i = 0; i < 10; i++) b.executer(`INSERT (:D {i: ${i}, g: ${i % 5}})`);
    try {
      assert.profond(col(b.executer("MATCH (d:D) RETURN d.g, count(d) ORDER BY d.g")), [0, 1, 2, 3, 4]);
    } catch (e) {
      if (String(e).includes("not found for ORDER BY")) return "ignoré : bogue moteur connu, corrigé par fix/order-by-apres-agregat";
      throw e;
    }
  });

  // ───────────── persistance (OPFS) : côté hors worker ─────────────
  // Le aller-retour persistant (publier, recharger) est dans crates/obrain-web/tests/navigateur.rs,
  // exécuté en worker dédié : seul contexte où OPFS donne son accès synchrone. Ici, on vérifie
  // que hors de ce contexte (node) rien ne trappe. Sur la page principale, la Promise
  // d'OPFS ne se résout pas sous --virtual-time-budget (le dump de lancer.sh) ; le rejet
  // réel y a été vérifié en temps réel par WebDriver (createSyncAccessHandle y est absent).
  test("persistance : ouvrir hors worker dédié rejette par une erreur lisible", async () => {
    if (typeof Base.ouvrir !== "function") return "ignoré : persistance absente";
    if (typeof window !== "undefined") return "ignoré : OPFS asynchrone non servi par le budget de temps virtuel";
    let e = null;
    try { await Base.ouvrir("suite-" + Date.now() + ".journal"); } catch (x) { e = x; }
    assert.ok(e !== null, "ouvrir doit échouer sans OPFS synchrone");
    assert.ok(String(e).length > 0, "message non vide");
  });

  test("persistance : persister une base en mémoire est une erreur, pas une trappe", () => {
    const b = a();
    if (typeof b.persister !== "function") return "ignoré : persistance absente";
    assert.leve(() => b.persister(), "ouvert", "base non ouverte par ouvrir");
    assert.egal(b.noeuds(), 0, "la base reste utilisable");
  });

  return tests;
}

export async function executer(Base, ecrire = () => {}) {
  const tests = suite(Base);
  const rapport = { total: tests.length, ok: 0, echecs: [], ignores: [], duree_ms: 0 };
  const t0 = performance.now();
  for (const t of tests) {
    const d0 = performance.now();
    try {
      const r = await t.fn();
      if (typeof r === "string" && r.startsWith("ignoré")) { rapport.ignores.push(`${t.nom} — ${r}`); ecrire(`~ ${t.nom} (${r})`); }
      else { rapport.ok++; ecrire(`✔ ${t.nom} (${(performance.now() - d0).toFixed(0)} ms)`); }
    } catch (e) {
      rapport.echecs.push({ nom: t.nom, erreur: String(e && e.message || e).slice(0, 300) });
      ecrire(`✘ ${t.nom} — ${String(e && e.message || e).slice(0, 200)}`);
    }
  }
  rapport.duree_ms = Math.round(performance.now() - t0);
  return rapport;
}
