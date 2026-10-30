/**
 * ── TESTS DE `flotte.ts` — Node pur, aucun framework, aucune dépendance ──────
 *
 * Ce qui est éprouvé ici n'est pas de la décoration : ce sont les trois règles
 * qui décident de ce que l'utilisateur CROIT — « cette machine est vivante »,
 * « cet outil m'est proposé », « c'est réussi ». Les valeurs d'entrée sont
 * celles du protocole RÉEL (`remote.rs`, `attache.rs`), recopiées du code :
 * une forme inventée ne prouverait rien sur le moteur qui tourne.
 *
 * Compilation puis exécution (depuis `packages/obrain-dashboard`) — la même
 * méthode que `segments.test.mjs` / `raccord.test.mjs` du dossier :
 *
 *   npx tsc src/graph/flotte.ts --outDir /tmp/flotte-test --module esnext \
 *     --target es2020 --moduleResolution bundler --strict \
 *     && node src/graph/flotte.test.mjs
 *
 * `flotte.ts` n'importe RIEN : c'est ce qui rend cette compilation possible, et
 * c'est aussi ce qui rend la règle éprouvable sans navigateur.
 */
import assert from "node:assert/strict";
import { existsSync, mkdirSync, writeFileSync } from "node:fs";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const SORTIE = process.env.FLOTTE_TEST_OUT ?? "/tmp/flotte-test";
mkdirSync(SORTIE, { recursive: true });
writeFileSync(join(SORTIE, "package.json"), '{ "type": "module" }\n');

const chemins = [
  join(SORTIE, "flotte.js"),
  join(SORTIE, "graph", "flotte.js"),
  join(SORTIE, "src", "graph", "flotte.js"),
];
const trouve = chemins.find((p) => existsSync(p));
assert.ok(
  trouve,
  `module compilé introuvable — attendu l'un de : ${chemins.join(", ")}\n` +
    `as-tu lancé « npx tsc src/graph/flotte.ts --outDir ${SORTIE} … » ?`,
);

const {
  SILENCE_OUBLI_MS,
  SEUIL_VIVANTE_MS,
  presenceDe,
  presencedAge,
  silenceDe,
  dureeLisible,
  capacitesAnnoncees,
  capacitesNonAnnoncees,
  capacitesHorsCatalogue,
  absencesLisible,
  etatDeLaListe,
  doitBasculerSurRouteHeritee,
  listeAffichable,
  FICHE_OUTILS,
  ficheDe,
  verifierAppel,
  construireArguments,
  lireVerdict,
  resumeVerdict,
  verdictInterface,
  joignable,
  motifNonJoignable,
  depuisLisible,
} = await import(pathToFileURL(trouve).href);

let faits = 0;
const verifier = (condition, quoi) => {
  faits++;
  console.log(`${condition ? "  ok  " : " FAIL "} ${quoi}`);
  assert.ok(condition, quoi);
};

// ── Un client, dans la forme EXACTE de `Client::en_json` (remote.rs) ────────
const client = (silence_ms, extra = {}) => ({
  id: "cl-18f2c0d1a40-1",
  machine: "portable-triv",
  capacites: ["read", "list", "write", "status"],
  attache_ms: 1_787_000_000_000,
  derniere_vue_ms: 1_787_000_090_000,
  silence_ms,
  ...extra,
});

console.log("\n── 1. LA PRÉSENCE : une machine qu'on n'a pas vue n'est pas vivante");
{
  verifier(silenceDe(client(1200)) === 1200, "le silence rendu par le moteur est retenu tel quel");

  const v = presenceDe(client(1200));
  verifier(v.presence === "vivante", `1,2 s de silence → vivante (${v.presence})`);
  verifier(v.age_ms === 1200, "l'âge affiché est celui du moteur");

  // Le long-poll du client vaut 20 s (ATTENTE_S), le moteur borne à 25 s :
  // 31 s couvre un tour de battement ENTIER perdu.
  const f = presenceDe(client(31_000));
  verifier(f.presence === "froide", `31 s de silence → froide, pas vivante (${f.presence})`);
  verifier(
    f.presence !== presenceDe(client(1200)).presence,
    "une machine froide est VISIBLEMENT distinguée d'une machine vivante",
  );

  // 90 s est le seuil d'oubli du MOTEUR lui-même (SILENCE_MAX_MS).
  verifier(
    presenceDe(client(SILENCE_OUBLI_MS)).presence === "froide",
    "à exactement 90 s, le moteur la garde encore : « froide », pas « muette »",
  );
  const m = presenceDe(client(SILENCE_OUBLI_MS + 1));
  verifier(m.presence === "muette", `au-delà de 90 s → muette (${m.presence})`);
  verifier(
    /liste périmée/.test(m.libelle),
    "et l'affichage dit que c'est la LISTE qui est périmée, pas la machine",
  );

  // Un silence ABSENT ne devient jamais « vivante » : c'est le mensonge le plus
  // facile à commettre (on comble l'absence par l'hypothèse rassurante).
  const sansMesure = presenceDe({ ...client(0), silence_ms: undefined });
  verifier(sansMesure.presence === "inconnue", `silence absent → inconnue (${sansMesure.presence})`);
  verifier(sansMesure.age_ms === null, "et aucun âge n'est inventé");
  verifier(
    presenceDe({ ...client(0), silence_ms: Number.NaN }).presence === "inconnue",
    "NaN n'est pas une mesure : inconnue (NaN traverse `!= null`)",
  );

  verifier(dureeLisible(4_000) === "il y a 4 s", `4 000 ms → « ${dureeLisible(4_000)} »`);
  verifier(dureeLisible(125_000) === "il y a 2 min", `125 000 ms → « ${dureeLisible(125_000)} »`);
  verifier(dureeLisible(Number.NaN) === "durée inconnue", "une durée non mesurable se dit");
  verifier(depuisLisible(125_000) === "2 min", `« depuis » se dit sans « il y a » : « ${depuisLisible(125_000)} »`);
}

console.log("\n── 1bis. CONNUE N'EST PAS JOIGNABLE (champ `vivant` du registre)");
{
  // ── LE CAS QUI COMPTE : après un redémarrage du moteur, une machine relue du
  // journal affiche un silence COURT — parce que le MOTEUR vient de redémarrer,
  // pas parce que la machine a parlé. Jugée sur ce seul silence, elle
  // s'afficherait « vivante » et l'utilisateur enverrait une demande que le
  // moteur refusera de joindre.
  const relue = { ...client(400), vivant: false };
  const p = presenceDe(relue);
  verifier(p.presence === "connue", `silence de 400 ms + vivant:false → « connue » (${p.presence})`);
  verifier(p.presence !== "vivante", "et surtout PAS « vivante », malgré 0,4 s de silence");
  verifier(/n'est pas joignable|pas joignable/.test(p.libelle), `l'affichage le dit : « ${p.libelle} »`);
  verifier(p.age_ms === 400, "l'âge reste affiché : « se tait depuis 0 s » est une information");

  verifier(joignable(relue) === false, "joignable() répond NON sur vivant:false");
  const motif = motifNonJoignable(relue);
  verifier(typeof motif === "string" && motif.length > 60, "et un motif actionnable existe");
  verifier(/Rallumez le nœud/.test(motif), "le motif dit QUOI FAIRE, pas seulement que c'est fermé");
  verifier(
    /vivant: false/.test(motif),
    "et il nomme sa source (le champ du registre) : le refus est vérifiable, pas une opinion de l'interface",
  );

  // ── LE MÊME SILENCE, MAIS VIVANT : c'est bien la machine qu'on juge ────────
  const vivante = { ...client(400), vivant: true };
  verifier(presenceDe(vivante).presence === "vivante", "vivant:true + 0,4 s → vivante");
  verifier(joignable(vivante) === true, "et joignable");
  verifier(motifNonJoignable(vivante) === null, "sans motif de refus");
  verifier(
    presenceDe(vivante).presence !== presenceDe(relue).presence,
    "deux machines au MÊME silence sont donc VISIBLEMENT distinguées",
  );

  // ── LE MONDE D'AUJOURD'HUI : le champ n'existe pas encore ------------------
  // Le 8432 sert un binaire construit avant `vivant`. On ne refuse pas sur une
  // absence d'information : on laisse le moteur trancher, avec un refus motivé.
  const sansChamp = client(400);
  verifier(joignable(sansChamp) === true, "vivant absent → on ne ferme pas la porte");
  verifier(motifNonJoignable(sansChamp) === null, "et aucun motif de refus n'est inventé");
  verifier(motifNonJoignable(null) === null, "aucune machine → pas de motif de non-joignabilité");
  verifier(presenceDe(sansChamp).presence === "vivante", "la classification reste celle du silence");
}

console.log("\n── 2. LES CAPACITÉS : on ne propose que ce qui est ANNONCÉ");
{
  // `capacites()` de attache.rs : read, list, write, status, puis exec SI et
  // seulement SI la politique locale l'autorise.
  const sansExec = client(500);
  const annoncees = capacitesAnnoncees(sansExec);
  verifier(annoncees.join(",") === "read,list,write,status", `annoncées : ${annoncees.join(",")}`);
  verifier(!annoncees.includes("exec"), "« exec » n'est PAS proposé sur un nœud sans --allow-exec");

  const avecExec = capacitesAnnoncees(client(500, { capacites: ["read", "list", "write", "status", "exec"] }));
  verifier(avecExec.includes("exec"), "il l'est sur un nœud qui l'annonce");

  const doublons = capacitesAnnoncees(client(500, { capacites: ["read", "read", " read ", "", null] }));
  verifier(doublons.join(",") === "read", `doublons et vides écartés, ordre gardé : ${doublons.join(",")}`);

  // ── LE REFUS QUI COMPTE : un outil NON annoncé ne part pas ───────────────
  const r = verifierAppel(sansExec, "exec", "{}");
  verifier(r.ok === false, "un appel « exec » vers un nœud qui ne l'annonce pas est REFUSÉ");
  verifier(/n'est pas annoncé/.test(r.motif), `et le motif nomme la cause : « ${r.motif.slice(0, 60)}… »`);
  verifier(
    /read, list, write, status/.test(r.motif),
    "le motif rappelle CE QUE la machine annonce : le refus est actionnable",
  );

  verifier(verifierAppel(null, "read", "{}").ok === false, "sans machine choisie, rien ne part");
  const sansOutil = verifierAppel(sansExec, "   ", "{}");
  verifier(sansOutil.ok === false, "sans outil, rien ne part");
  verifier(
    /« outil » requis/.test(sansOutil.motif),
    "et le motif reprend celui du moteur (400 « « outil » requis » de api_remote)",
  );

  verifier(verifierAppel(sansExec, "read", '{"path":"a.md"}').ok === true, "un outil annoncé passe");
  verifier(verifierAppel(sansExec, "read", "").ok === true, "arguments vides = objet vide, pas une erreur");

  const mal = verifierAppel(sansExec, "read", "{pas du json");
  verifier(mal.ok === false, "un JSON illisible est refusé AVANT le réseau");
  const tableau = verifierAppel(sansExec, "read", '["path"]');
  verifier(tableau.ok === false, "un tableau n'est pas un objet d'arguments");
  verifier(/OBJET JSON/.test(tableau.motif), "et le motif dit la forme attendue");

  // Les fiches viennent de `outils_modele()` (agent.rs) : les champs requis y
  // sont déclarés, et un champ requis vide ne doit pas partir.
  verifier(ficheDe("read")?.champs.find((c) => c.nom === "path")?.requis === true, "read → path requis");
  verifier(ficheDe("exec")?.champs.find((c) => c.nom === "cmd")?.requis === true, "exec → cmd requis");
  verifier(ficheDe("status")?.champs.length === 0, "status ne prend aucun argument");
  verifier(ficheDe("outil_du_futur") === null, "un outil inconnu de cette version n'a pas de fiche");

  const manquant = construireArguments("read", {});
  verifier(manquant.ok === false, "champ requis vide → refus local, motif clair");
  verifier(/« path » est requis/.test(manquant.motif), `motif : « ${manquant.motif} »`);

  const ok = construireArguments("read", { path: " notes/a.md ", maxBytes: "" });
  verifier(ok.ok === true, "read complet passe");
  verifier(ok.arguments.path === "notes/a.md", "le chemin est détouré de ses espaces");
  verifier(!("maxBytes" in ok.arguments), "un champ optionnel VIDE n'est pas envoyé (on ne change pas le défaut du nœud)");

  const ex = construireArguments("exec", { cmd: "df", args: "-h\n/\n\n", shell: "true", cwd: "" });
  verifier(ex.ok === true, "exec complet passe");
  verifier(Array.isArray(ex.arguments.args) && ex.arguments.args.length === 2, "args : une ligne = un argument, les vides écartés");
  verifier(ex.arguments.shell === true, "shell est envoyé en BOOLÉEN (le nœud fait as_bool)");
  verifier(!("cwd" in ex.arguments), "cwd vide n'est pas envoyé");

  const entier = construireArguments("read", { path: "a.md", maxBytes: "abc" });
  verifier(entier.ok === false, `maxBytes non numérique → refus local (${entier.motif})`);
  const entierOk = construireArguments("read", { path: "a.md", maxBytes: "4096" });
  verifier(entierOk.arguments.maxBytes === 4096, "maxBytes numérique devient un NOMBRE, pas une chaîne");
  verifier(Object.keys(FICHE_OUTILS).length === 5, "cinq capacités annoncées par selfd ont leur fiche");
}

console.log("\n── 3. LE VERDICT : succès, refus motivé et délai sont TROIS choses");
{
  // (a) Le nœud a exécuté — forme EXACTE de `attache.rs` : {"ok":true,"resultat":…}
  const s = lireVerdict(200, JSON.stringify({ resultat: { ok: true, resultat: { code: 0, stdout: "…" } } }));
  verifier(s.genre === "succes", `200 + ok:true → succès (${s.genre})`);
  verifier(!!s.resultat, "la charge utile du nœud est conservée");

  // (b) La politique LOCALE du nœud a refusé — {"ok":false,"refus":"<motif>"}
  const refus = lireVerdict(
    200,
    JSON.stringify({ resultat: { ok: false, refus: "chemin hors de la racine autorisée (/srv/notes)" } }),
  );
  verifier(refus.genre === "refus_machine", `200 + ok:false → refus MACHINE (${refus.genre})`);
  verifier(
    /hors de la racine autorisée/.test(refus.detail),
    "le MOTIF du refus est affiché, pas résumé en « erreur »",
  );
  verifier(
    /politique LOCALE/.test(refus.detail),
    "et il est attribué à qui a tranché : la politique du nœud, pas le moteur",
  );

  // (c) Le MOTEUR a renoncé : message exact de `remote::appeler`
  const delai = lireVerdict(409, JSON.stringify({ error: "le client n'a pas répondu en 30 s" }));
  verifier(delai.genre === "delai", `409 « n'a pas répondu en 30 s » → DÉLAI (${delai.genre})`);
  verifier(
    /retiré sa demande/.test(delai.detail),
    "et l'affichage dit que la demande a été RETIRÉE (elle ne s'exécutera pas plus tard)",
  );

  // (d) Le moteur a refusé d'ÉMETTRE : il n'y avait personne. Même code HTTP,
  //     cause opposée — les confondre enverrait rallumer une machine éteinte.
  const personne = lireVerdict(409, JSON.stringify({ error: "aucun client de ce nom n'est connecté" }));
  verifier(personne.genre === "refus_moteur", `409 « aucun client » → refus moteur (${personne.genre})`);
  verifier(personne.genre !== delai.genre, "« personne » et « pas de réponse » sont DEUX affichages distincts");
  verifier(
    personne.genre !== refus.genre,
    "et ni l'un ni l'autre ne se confond avec un refus de la machine",
  );
  verifier(
    /n'est pas à vous/.test(lireVerdict(409, JSON.stringify({ error: "ce client n'est pas à vous" })).detail),
    "« ce client n'est pas à vous » remonte aussi, non masqué",
  );

  // (e) La session a expiré : 401 du garde IAM. On ne le présente PAS comme un
  //     échec de la machine.
  const sansIdentite = lireVerdict(
    401,
    JSON.stringify({ error: "aucune identité présentée (en-tête `Authorization: Bearer <jeton>`, cookie `obrain_session`, ou `?token=`) : l'IAM est armé." }),
  );
  verifier(sansIdentite.genre === "refus_moteur", "401 → refus moteur, pas un échec de la machine");
  verifier(/reconnectez-vous/.test(sansIdentite.detail), "et il dit quoi FAIRE");

  // (f) 200 sans les champs du protocole : on n'affirme AUCUN succès.
  const brut = lireVerdict(200, JSON.stringify({ resultat: { tables: 3 } }));
  verifier(brut.genre === "verdict_brut", `200 de forme inconnue → verdict_brut (${brut.genre})`);
  verifier(brut.genre !== "succes", "« pas reconnu » n'est pas « réussi »");

  // (g) Corps illisible : le brut est conservé, et le genre ne ment pas.
  const pasJson = lireVerdict(200, "<html>502 Bad Gateway</html>");
  verifier(pasJson.genre === "verdict_brut", "un corps non-JSON en 200 n'est pas un succès");
  verifier(/502 Bad Gateway/.test(pasJson.brut), "le corps brut est conservé pour le rapport");

  const cinqCents = lireVerdict(500, "boom");
  verifier(cinqCents.genre === "transport", `500 → transport, verdict INCONNU (${cinqCents.genre})`);
  verifier(/INCONNU/.test(cinqCents.detail), "et l'affichage refuse explicitement de trancher");

  // Le journal garde une ligne lisible, bornée. La borne est `<= 160` et non
  // `=== 160` : le point de coupe est un détail d'implémentation, la propriété
  // qui compte est qu'une ligne de journal ne puisse pas peser un paragraphe.
  const long = resumeVerdict({ genre: "delai", detail: "x".repeat(500), titre: "", couleur: "" });
  verifier(long.length <= 160, `le résumé de journal est borné (${long.length} car. sur 500)`);
  verifier(/…$/.test(long), "et tronqué visiblement");
  const court = resumeVerdict({ genre: "succes", detail: "court", titre: "", couleur: "" });
  verifier(court === "court", "un détail court n'est pas touché");

  // ── LE REFUS DE L'INTERFACE N'EST PAS UN REFUS DU MOTEUR ────────────────
  // Rien n'est parti sur le réseau : l'attribuer au moteur ferait chercher dans
  // les journaux du nœud une trace qui n'existe pas.
  const local = verdictInterface("« exec » n'est pas annoncé par « portable-triv »");
  verifier(local.genre === "refus_interface", `refus local → genre propre (${local.genre})`);
  verifier(
    local.genre !== refus.genre && local.genre !== delai.genre && local.genre !== personne.genre,
    "et il ne se confond avec AUCUNE des trois réponses du protocole",
  );
  verifier(/rien n'a été envoyé/.test(local.titre), `le titre le dit : « ${local.titre} »`);
  verifier(verdictInterface("").detail === "motif absent", "un motif vide ne devient pas un vide affiché");
}

// ════════════════════════════════════════════════════════════════════════════
// SECOND PASSAGE (2026-09-16) : la route qui RÉPOND, l'âge qui vieillit, et ce
// qu'une machine NE fait pas. Les formes JSON sont recopiées du
// `serde_json::json!` de la route `/api/remote/machines` (viz_serve.rs).
// ════════════════════════════════════════════════════════════════════════════

// ── Le corps EXACT de `GET /api/remote/machines` ────────────────────────────
const corpsMachines = (extra = {}) =>
  JSON.stringify({
    compte: "alice",
    question: "quelles machines connais-tu, et que sait faire chacune ?",
    machines: [],
    connues: 0,
    joignables: 0,
    aucune: true,
    reponse: "ce compte ne connaît AUCUNE machine : …",
    ...extra,
  });

console.log("\n── 4. LA ROUTE DE LA FLOTTE : ce que le moteur AFFIRME, et ce qu'il tait");
{
  // (a) Deux machines, dont une seule vivante — la forme réelle de la route.
  const deux = etatDeLaListe(
    200,
    corpsMachines({
      machines: [
        { ...client(1200), vivant: true },
        { ...client(4000), machine: "vieux-portable", vivant: false },
      ],
      connues: 2,
      joignables: 1,
      aucune: false,
      reponse: "2 machine(s) connue(s) de ce compte, 1 joignable(s) ; les autres sont connues du journal.",
    }),
  );
  verifier(deux.etat === "liste", `200 + machines → « liste » (${deux.etat})`);
  verifier(deux.machines.length === 2, "les deux machines sont nommées, aucune n'est inventée");
  verifier(deux.question !== null, "la QUESTION du moteur est conservée (elle dit ce que la liste couvre)");
  verifier(
    deux.accordCompteurs === "verifie",
    `les compteurs du moteur s'accordent avec sa liste (${deux.accordCompteurs})`,
  );
  verifier(deux.compteurs.connues === 2 && deux.comptees.total === 2, "compteurs ET comptage local disent 2");
  verifier(deux.comptees.joignables === 1, "l'interface compte 1 joignable (vivant:true) sur 2");
  verifier(
    presenceDe(deux.machines[0]).presence !== presenceDe(deux.machines[1]).presence,
    "vivante et « connue, pas joignable » sont DEUX affichages dans la même liste",
  );

  // (b) LE VIDE AFFIRMÉ — la seule liste vide qui a le droit d'être une réponse.
  const aucune = etatDeLaListe(200, corpsMachines());
  verifier(aucune.etat === "aucune", `200 + aucune:true → « aucune » (${aucune.etat})`);
  verifier(aucune.aucune === true, "le champ `aucune` du moteur est retenu, pas déduit");
  verifier(
    aucune.message === "ce compte ne connaît AUCUNE machine : …",
    "et la phrase `reponse` est citée MOT POUR MOT (elle nomme la commande, donc elle ne se périme pas dans une page)",
  );

  // (c) LE VIDE NON AFFIRMÉ — le défaut que la doctrine interdit de maquiller.
  //     Sans `aucune` ni `reponse`, une liste vide n'est pas un fait : c'est un
  //     silence, et il rend un écran DIFFÉRENT de « aucune machine ».
  const muet = etatDeLaListe(200, JSON.stringify({ compte: "alice", machines: [], connues: 0, joignables: 0 }));
  verifier(muet.etat === "vide_non_affirme", `200 + liste vide sans le champ "aucune" → « vide_non_affirme » (${muet.etat})`);
  verifier(muet.etat !== aucune.etat, "« liste vide non affirmée » et « aucune machine » ne se confondent PAS");
  verifier(/ne l'AFFIRME pas/.test(muet.message), `et l'affichage le dit : « ${muet.message.slice(0, 70)}… »`);

  // (d) Le moteur se CONTREDIT : `aucune: false` avec une liste vide. On ne
  //     choisit pas l'interprétation flatteuse ; on dit la contradiction.
  const contredit = etatDeLaListe(200, corpsMachines({ aucune: false }));
  verifier(contredit.etat === "vide_non_affirme", `aucune:false + [] → vide non affirmé (${contredit.etat})`);
  verifier(/aucune: false/.test(contredit.message), "et le message nomme la contradiction observée");

  // (e) 404 : la route n'existe pas sur ce moteur. C'est le seul état qui
  //     autorise à relire la route héritée — et jamais en silence.
  const absente = etatDeLaListe(404, JSON.stringify({ error: "route inconnue du canal" }));
  verifier(absente.etat === "route_absente", `404 → route_absente (${absente.etat})`);
  verifier(absente.etat !== aucune.etat, "« route absente » n'est PAS « aucune machine »");
  verifier(doitBasculerSurRouteHeritee(absente) === true, "sur 404 seulement, on relit la route héritée");
  verifier(/404/.test(absente.message) && /route inconnue du canal/.test(absente.message), "le motif du moteur est remonté tel quel");

  // (f) Le refus d'identité : il ne dit RIEN des machines — donc il ne doit pas
  //     produire un écran vide, qui se lirait « aucune machine ».
  const sansIdentite = etatDeLaListe(401, JSON.stringify({ error: "aucune identité présentée" }));
  verifier(sansIdentite.etat === "identite_refusee", `401 → identite_refusee (${sansIdentite.etat})`);
  verifier(/aucune identité présentée/.test(sansIdentite.message), "le refus est MOTIVÉ (le silence ressemblerait à une panne)");
  verifier(/Reconnectez-vous/.test(sansIdentite.message), "et il dit quoi FAIRE");
  verifier(sansIdentite.etat !== aucune.etat, "un refus d'identité ne se confond pas avec une flotte vide");

  // (g) 200 ne prouve rien : un mandataire ou une page rendent aussi 200.
  const html = etatDeLaListe(200, "<!doctype html><html><body>Obrain</body></html>");
  verifier(html.etat === "corps_illisible", `200 + HTML → corps_illisible (${html.etat})`);
  verifier(html.etat !== aucune.etat, "et surtout pas « aucune machine »");
  const autreRoute = etatDeLaListe(200, JSON.stringify({ tables: 3 }));
  verifier(autreRoute.etat === "corps_illisible", "200 sans champ `machines` → corps_illisible");
  const entetes = etatDeLaListe(200, JSON.stringify({ machines: [42, "portable"] }));
  verifier(entetes.etat === "corps_illisible", "des entrées qui ne sont pas des machines → rien n'est affiché");

  // (h) Toute autre erreur : le motif du moteur remonte, non résumé.
  const cinqCents = etatDeLaListe(500, JSON.stringify({ error: "registre illisible" }));
  verifier(cinqCents.etat === "erreur_moteur", `500 → erreur_moteur (${cinqCents.etat})`);
  verifier(/registre illisible/.test(cinqCents.message), "le motif est remonté");
  verifier(!doitBasculerSurRouteHeritee(cinqCents), "on ne bascule PAS sur une erreur : la route a parlé");

  // (i) Les compteurs du moteur sont CONFRONTÉS à sa propre liste.
  const desaccord = etatDeLaListe(
    200,
    corpsMachines({
      machines: [{ ...client(1000), vivant: true }],
      connues: 3,
      joignables: 2,
      aucune: false,
    }),
  );
  verifier(desaccord.accordCompteurs === "desaccord", `compteurs vs liste → désaccord (${desaccord.accordCompteurs})`);
  verifier(
    /déclare 3 machine\(s\) connue\(s\) mais en nomme 1/.test(desaccord.motifCompteurs),
    `le désaccord est nommé, pas tu : « ${desaccord.motifCompteurs} »`,
  );
  verifier(
    /déclare 2 joignable\(s\)/.test(desaccord.motifCompteurs),
    "et le second écart (joignables) est dit aussi",
  );

  // (j) Un moteur antérieur : pas de `vivant` par machine. On ne compte pas ce
  //     qu'on ne peut pas distinguer — et on ne prétend pas avoir vérifié.
  const sansVivant = etatDeLaListe(
    200,
    corpsMachines({ machines: [client(1000)], connues: 1, joignables: 1, aucune: false }),
  );
  verifier(sansVivant.comptees.joignables === null, "sans champ `vivant`, l'interface ne compte AUCUN joignable");
  verifier(
    sansVivant.accordCompteurs === "non_verifiable",
    `et le compte du moteur est déclaré non vérifiable (${sansVivant.accordCompteurs})`,
  );

  // (k) LA ROUTE HÉRITÉE : elle liste, mais elle n'affirme rien. Une liste vide
  //     lue par elle n'est donc PAS « aucune machine ».
  const heriteeVide = etatDeLaListe(200, JSON.stringify({ compte: "alice", clients: [], aide: "selfd attacher …" }), "clients");
  verifier(heriteeVide.heritee === true, "la lecture dit qu'elle vient de la route héritée");
  verifier(heriteeVide.aucune === null, "cette route ne porte aucun champ `aucune`");
  verifier(
    heriteeVide.etat === "vide_non_affirme",
    `une liste vide par la route héritée n'est PAS « aucune machine » (${heriteeVide.etat})`,
  );
  verifier(
    heriteeVide.accordCompteurs === "non_verifiable",
    "et ses compteurs sont déclarés non vérifiables (elle n'en rend aucun)",
  );
  verifier(
    !doitBasculerSurRouteHeritee(heriteeVide),
    "on ne rebascule pas indéfiniment : la bascule n'a lieu que sur la route de la flotte",
  );

  // (l) Le champ de liste compte : la route de la flotte lue avec le nom
  //     `clients` (ou l'inverse) est un corps illisible, pas une flotte vide.
  const mauvaisChamp = etatDeLaListe(200, JSON.stringify({ compte: "alice", clients: [client(1000)] }));
  verifier(
    mauvaisChamp.etat === "corps_illisible",
    `la route « machines » ne lit pas le champ « clients » (${mauvaisChamp.etat})`,
  );
  const heriteeOk = etatDeLaListe(200, JSON.stringify({ compte: "alice", clients: [client(1000)] }), "clients");
  verifier(heriteeOk.etat === "liste" && heriteeOk.machines.length === 1, "mais elle le lit quand on le lui demande");
}

console.log("\n── 5. L'ÂGE ENTRE DEUX LECTURES : une mesure datée doit vieillir");
{
  // ── LE DÉFAUT CORRIGÉ ─────────────────────────────────────────────────────
  // Le moteur a mesuré 1 s de silence AU MOMENT de sa réponse. L'onglet, lui,
  // est passé en arrière-plan : le navigateur étrangle ses minuteries, la liste
  // n'a pas été relue depuis 45 s. Sans vieillissement, l'écran afficherait
  // « vivante » sur une mesure de 45 s — c'est-à-dire une affirmation qu'il n'a
  // plus vérifiée.
  const mesure = client(1000);
  verifier(presenceDe(mesure).presence === "vivante", "à la lecture : vivante (1 s de silence)");
  const vieillie = presenceDe(mesure, 45_000);
  verifier(vieillie.presence === "froide", `45 s plus tard, SANS relecture : « froide » (${vieillie.presence})`);
  verifier(vieillie.age_ms === 46_000, `l'âge retenu = mesure + écoulé (${vieillie.age_ms} ms)`);
  verifier(vieillie.mesure_ms === 1000, "la mesure du moteur reste consultable (elle n'est pas écrasée)");
  verifier(vieillie.ecoule_ms === 45_000, "et le temps écoulé depuis la lecture est dit");
  // Le suffixe ne se pose que là où il change la lecture : un « vivante » qui a
  // vieilli doit dire À QUOI il se rapporte. « froide »/« muette » disent déjà
  // l'ancienneté par leurs mots ; y ajouter le suffixe serait du bruit.
  const encoreVivante = presenceDe(mesure, 20_000);
  verifier(encoreVivante.presence === "vivante", `1 s + 20 s écoulés = 21 s : encore vivante (${encoreVivante.presence})`);
  verifier(
    /dernière lecture/.test(encoreVivante.libelle),
    `et le libellé dit à quoi se rapporte le « vivante » : « ${encoreVivante.libelle} »`,
  );
  verifier(!/dernière lecture/.test(presenceDe(mesure).libelle), "une lecture fraîche n'ajoute pas ce bruit");

  verifier(
    presenceDe(mesure, 0).presence === presenceDe(mesure).presence,
    "sans écoulé, le comportement d'avant est conservé (compatibilité de l'appel)",
  );
  verifier(presencedAge(null, 5000) === null, "aucune mesure → aucun âge, même avec du temps écoulé");
  verifier(presencedAge(1000, 5000) === 6000, "deux DURÉES s'additionnent (jamais deux dates)");

  // Une horloge locale qui recule (mise à l'heure, réveil de veille) ne doit pas
  // Rajeunir une machine : un écoulé négatif vaut 0.
  verifier(presencedAge(2000, -9000) === 2000, "un écoulé négatif ne rajeunit pas la machine");
  verifier(presencedAge(2000, Number.NaN) === 2000, "un écoulé non mesurable ne change rien");

  // Au-delà du seuil d'oubli du MOTEUR, c'est la LISTE qui est périmée.
  const oubliee = presenceDe(client(1000), SILENCE_OUBLI_MS + 5_000);
  verifier(oubliee.presence === "muette", `97 s sans relecture → « muette » (${oubliee.presence})`);
  verifier(/liste périmée/.test(oubliee.libelle), "et l'affichage attribue le retard à la LISTE, pas à la machine");

  // `vivant: false` reste plus fort que le vieillissement : c'est un fait du
  // registre, aucune durée locale ne le contredit.
  const connue = presenceDe({ ...client(400), vivant: false }, 20_000);
  verifier(connue.presence === "connue", `vivant:false + 20 s → toujours « connue » (${connue.presence})`);
}

console.log("\n── 6. CE QU'UNE MACHINE NE FERA PAS — l'absence est une information");
{
  const sansExec = client(500);
  const absentes = capacitesNonAnnoncees(sansExec);
  verifier(absentes.length === 1 && absentes[0].outil === "exec", `une seule absence annoncée : ${absentes.map((a) => a.outil).join(",")}`);
  verifier(
    /AUCUN programme/.test(absentes[0].consequence),
    `et sa CONSÉQUENCE est dite : « ${absentes[0].consequence} »`,
  );

  const phrase = absencesLisible(sansExec);
  verifier(/n'annonce PAS : exec/.test(phrase), `la phrase nomme l'outil absent : « ${phrase.slice(0, 46)}… »`);
  verifier(/--allow-exec/.test(phrase), "et elle nomme la CAUSE (la politique du nœud, fixée à son démarrage)");
  verifier(
    /ne propose jamais un outil qu'une machine n'annonce pas/.test(phrase),
    "et elle rappelle la règle qui en découle",
  );

  // ── LE LIEN QUI COMPTE : ce qui est annoncé absent ne peut PAS partir ─────
  // L'affichage dit « exec ne lui sera pas proposé » ; le garde refuse `exec`.
  // Si les deux divergeaient, l'écran mentirait dans un sens ou dans l'autre.
  for (const m of absentes) {
    const r = verifierAppel(sansExec, m.outil, '{"cmd":"df"}');
    verifier(r.ok === false, `« ${m.outil} » affiché absent est REFUSÉ par verifierAppel`);
    verifier(/n'est pas annoncé/.test(r.motif), "et le refus nomme la même cause que l'affichage");
  }

  const complet = client(500, { capacites: ["read", "list", "write", "status", "exec"] });
  verifier(capacitesNonAnnoncees(complet).length === 0, "une machine qui annonce tout ne rend AUCUNE absence");
  verifier(absencesLisible(complet) === null, "et l'écran n'affiche alors aucune ligne d'absence (pas de bruit)");

  const rien = client(500, { capacites: [] });
  verifier(capacitesNonAnnoncees(rien).length === 5, "une machine qui n'annonce rien rend les cinq absences");
  verifier(
    /n'annonce AUCUNE capacité/.test(absencesLisible(rien)),
    "et la phrase le dit avant de détailler les conséquences",
  );

  // Une capacité que CETTE version de l'interface ne connaît pas est annoncée
  // sans fiche : elle doit être PROPOSÉE (JSON libre) et signalée comme telle.
  const futur = client(500, { capacites: ["read", "list", "write", "status", "instantane"] });
  verifier(
    capacitesHorsCatalogue(futur).join(",") === "instantane",
    "une capacité hors catalogue est repérée, donc affichée à part",
  );
  verifier(
    capacitesNonAnnoncees(futur).length === 1,
    "et elle n'est PAS comptée comme une absence : exec reste la seule",
  );
  verifier(verifierAppel(futur, "instantane", "{}").ok === true, "elle peut partir (champ JSON libre) : aucune machine n'est enfermée dans le catalogue");

  verifier(capacitesNonAnnoncees(null).length === 0, "aucune machine → aucune absence reprochée");
  verifier(absencesLisible(null) === null, "et aucune phrase d'absence");
}

console.log("\n── 7. QUELLE LISTE MONTRER : le cache ne contredit jamais une réponse fraîche");
{
  const m1 = { ...client(1000), vivant: true };
  const frais = etatDeLaListe(
    200,
    corpsMachines({ machines: [m1], connues: 1, joignables: 1, aucune: false }),
  );
  const memo = { machines: [{ ...client(999_000), machine: "fantome" }], le: 500_000 };

  const aff = listeAffichable(frais, memo, 700_000);
  verifier(aff.machines.length === 1 && aff.machines[0].machine === "portable-triv", "une lecture réussie REMPLACE le cache");
  verifier(aff.perimee === false, "et elle n'est pas marquée périmée");
  verifier(aff.le === 700_000, "sa date est celle de la lecture qui l'a rendue");

  // LE CAS QUI COMPTE : la réponse fraîche dit « aucune machine ». Elle gagne.
  const videAffirme = etatDeLaListe(200, corpsMachines());
  const affVide = listeAffichable(videAffirme, memo, 700_000);
  verifier(affVide.machines.length === 0, "« aucune machine » (affirmée) efface le cache : le cache ne contredit pas la réponse");
  verifier(affVide.perimee === false, "et rien n'est présenté comme périmé, puisque rien n'est montré");

  const nonAffirme = etatDeLaListe(200, JSON.stringify({ compte: "alice", machines: [] }));
  verifier(
    listeAffichable(nonAffirme, memo, 1).machines.length === 0,
    "un vide NON affirmé efface aussi la liste : un défaut de route n'est pas un état de la flotte",
  );

  // La lecture échoue : on garde la dernière liste connue, MARQUÉE périmée.
  const refuse = etatDeLaListe(401, JSON.stringify({ error: "aucune identité présentée" }));
  const affPerimee = listeAffichable(refuse, memo, 700_000);
  verifier(affPerimee.perimee === true, "sur un refus, la dernière liste connue est conservée");
  verifier(affPerimee.machines[0].machine === "fantome", "c'est bien l'ancienne liste qui est montrée");
  verifier(affPerimee.le === 500_000, "et sa date est celle de la lecture qui l'a rendue, pas celle de la tentative ratée");
  verifier(/DERNIÈRE CONNUE/.test(affPerimee.motif), `avec le motif qui le dit : « ${affPerimee.motif.slice(0, 40)}… »`);

  const rienDuTout = listeAffichable(refuse, null, 700_000);
  verifier(rienDuTout.machines.length === 0 && rienDuTout.perimee === false, "sans rien de connu, rien n'est affiché ni prétendu");
  verifier(listeAffichable(null, memo, 700_000).perimee === true, "transports en échec : la dernière liste connue reste, périmée");

  // ── LE TRANSPORT A LÂCHÉ APRÈS UNE LECTURE RÉUSSIE ────────────────────────
  // La liste affichée est celle de la dernière réponse HTTP — donc juste —, mais
  // plus personne ne l'a revérifiée. La présenter comme fraîche ferait dire
  // « vivante » à une machine dont on n'a plus de nouvelles depuis l'échec.
  const apresEchecReseau = listeAffichable(frais, memo, 700_000, true);
  verifier(
    apresEchecReseau.machines[0].machine === "portable-triv",
    "une lecture réussie suivie d'une panne réseau garde la liste reçue",
  );
  verifier(apresEchecReseau.perimee === true, `mais elle est marquée PÉRIMÉE (${apresEchecReseau.perimee})`);
  verifier(
    /DERNIÈRE CONNUE/.test(apresEchecReseau.motif),
    "et le motif dit qu'elle n'est plus vérifiée, pas qu'elle est fausse",
  );
  verifier(
    listeAffichable(frais, memo, 700_000, false).perimee === false,
    "sans échec de transport, la même lecture reste fraîche",
  );
  verifier(
    listeAffichable(videAffirme, memo, 700_000, true).machines.length === 0,
    "et une flotte affirmée vide reste vide : rien à marquer périmé",
  );
}

console.log(
  `\n── ${faits} vérifications — flotte (présence · âge · route de la flotte · capacités annoncées ET absentes · verdicts)`,
);
