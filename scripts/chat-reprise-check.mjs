// VÉRIFICATION DE LA REPRISE D'UN FLUX COUPÉ — le défaut refusé par l'utilisateur
//
//   « flux interrompu avant la fin — ce qui précède est incomplet »
//
// Ce que ce script mesure, et pourquoi il existe : la vue du chat (port 8432) ne
// doit JAMAIS avoir à écrire cette phrase. Quand le transport meurt en pleine
// réponse (le service `viz serve` est tué par un déploiement — journal systemd :
// `status=9/KILL`), la réponse doit CONTINUER, pas s'arrêter sur un aveu
// d'inachèvement.
//
// Le test ne simule rien : il interroge le VRAI serveur, avec le VRAI modèle, et
// il tue le service pour de bon (`--tuer`) — systemd le relance en ~2 s.
//
// Usage :
//   node scripts/chat-reprise-check.mjs                 (protocole seul, sans tuer)
//   node scripts/chat-reprise-check.mjs --tuer          (coupure RÉELLE en plein flux)
//   node scripts/chat-reprise-check.mjs --base URL --store NOM --q QUESTION
//
// Sortie : une ligne par fait mesuré, puis PASS ou FAIL (code de sortie 1 si FAIL).

import { request } from "node:http";
import { execFileSync } from "node:child_process";
import { ouvrirSession } from "./session-iam.mjs";

const args = process.argv.slice(2);
const lire = (nom, defaut) => {
  const i = args.indexOf(nom);
  return i >= 0 && args[i + 1] ? args[i + 1] : defaut;
};
const BASE = lire("--base", "http://192.168.1.103:8432");
const STORE = lire("--store", "Runtime dev (engrams)");
const QUESTION = lire("--q", "Compte de 1 à 20, en chiffres, séparés par des virgules.");
const TUER = args.includes("--tuer");

const { hostname, port } = new URL(BASE);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const faits = [];
const noter = (ok, quoi) => { faits.push({ ok, quoi }); console.log(`${ok ? "  ok  " : " FAIL "} ${quoi}`); };

/** Session IAM (vide si le serveur est ouvert) — voir `session-iam.mjs`. */
let SESSION = { enTetes: () => ({}) };

/** Une requête GET simple (sondage du serveur), avec l'identité si exigée. */
function get(chemin, timeoutMs = 5000) {
  return new Promise((res, rej) => {
    const req = request(
      { hostname, port, path: chemin, method: "GET", timeout: timeoutMs, headers: SESSION.enTetes() },
      (r) => {
        let corps = "";
        r.on("data", (c) => { corps += c; });
        r.on("end", () => res({ status: r.statusCode, corps }));
      },
    );
    req.on("timeout", () => { req.destroy(new Error("délai")); });
    req.on("error", rej);
    req.end();
  });
}

const serveurRepond = async () => {
  try { return (await get("/api/stores", 3000)).status === 200; } catch { return false; }
};

const attendreServeur = async (limiteMs) => {
  const t0 = Date.now();
  while (Date.now() - t0 < limiteMs) {
    if (await serveurRepond()) return Date.now() - t0;
    await sleep(250);
  }
  return null;
};

/**
 * Ouvre un flux de chat et rend ce qui a été observé.
 *
 * `deja` = le texte DÉJÀ produit : le serveur doit CONTINUER la réponse.
 * `arreterApres` = nombre de caractères de texte à partir duquel on coupe la
 * lecture (c'est ainsi qu'on « attrape » une réponse en plein vol).
 */
function ouvrirFlux({ deja, arreterApres = 0, limiteMs = 180_000 }) {
  const params = new URLSearchParams({ store: STORE, q: QUESTION });
  if (deja) params.set("deja", deja);
  return new Promise((res) => {
    const obs = { texte: "", think: "", fin: null, erreur: null, coupe: false, ms: null };
    const t0 = Date.now();
    const req = request(
      { hostname, port, path: `/api/engine/chat?${params}`, method: "GET", headers: SESSION.enTetes() },
      (r) => {
        obs.status = r.statusCode;
        let tampon = "";
        const finir = (coupe) => {
          obs.coupe = coupe;
          obs.ms = Date.now() - t0;
          try { req.destroy(); } catch { /* déjà fermé */ }
          res(obs);
        };
        r.on("data", (bloc) => {
          tampon += bloc.toString("utf8");
          let i;
          while ((i = tampon.indexOf("\n\n")) >= 0) {
            const evenement = tampon.slice(0, i);
            tampon = tampon.slice(i + 2);
            const nom = /^event: (.+)$/m.exec(evenement)?.[1]?.trim();
            const donnees = /^data: (.+)$/m.exec(evenement)?.[1];
            if (!nom || !donnees) continue;
            let d = null;
            try { d = JSON.parse(donnees); } catch { /* brut */ }
            if (nom === "texte" && d?.t) {
              obs.texte += d.t;
              if (arreterApres && obs.texte.length >= arreterApres) return finir(true);
            } else if (nom === "think" && d?.t) {
              obs.think += d.t;
            } else if (nom === "fin") {
              obs.fin = d;
              return finir(false);
            } else if (nom === "erreur") {
              obs.erreur = d?.error ?? donnees;
              return finir(false);
            }
          }
        });
        // Une coupure de transport se voit ICI : la réponse se termine sans `fin`.
        r.on("end", () => finir(false));
        r.on("error", () => finir(true));
      },
    );
    req.on("error", () => res({ status: 0, texte: "", think: "", fin: null, erreur: "connexion refusée", coupe: true, ms: 0 }));
    req.end();
    setTimeout(() => { try { req.destroy(); } catch { /* déjà fermé */ } }, limiteMs);
  });
}

/** Le service est TUÉ pour de bon : c'est la cause mesurée du défaut. */
function tuerLeService() {
  const pids = execFileSync("pgrep", ["-f", "obrain vi[z] serve"], { encoding: "utf8" })
    .split("\n").map((s) => s.trim()).filter(Boolean);
  for (const pid of pids) {
    try { process.kill(Number(pid), "SIGKILL"); } catch { /* déjà mort */ }
  }
  return pids.length;
}

// ─────────────────────────────────────────────────────────────────────────────

console.log(`── reprise de flux — ${BASE} · store « ${STORE} »${TUER ? " · COUPURE RÉELLE DEMANDÉE" : ""}`);

// ── L'IDENTITÉ D'ABORD : un serveur armé rend 401 sans session ──────────────
SESSION = await ouvrirSession(BASE, {
  utilisateur: lire("--user", null),
  motDePasse: lire("--password", null),
  identifiants: lire("--identifiants", undefined),
});
console.log(`  …    ${SESSION.note}`);

const attente = await attendreServeur(30_000);
if (attente === null) {
  console.log("  FAIL le serveur ne répond pas — rien à vérifier");
  process.exit(1);
}
noter(true, `serveur présent (${attente} ms)`);

// ── PHASE A : une réponse en plein vol ──────────────────────────────────────
const a = await ouvrirFlux({ arreterApres: TUER ? 60 : 0 });
noter(a.status === 200, `flux ouvert (HTTP ${a.status})`);
noter(a.texte.length > 0, `phase A : ${a.texte.length} caractères de réponse reçus`);
console.log(`        début : ${JSON.stringify(a.texte.slice(0, 80))}`);

let partiel = a.texte;
if (TUER) {
  noter(a.fin === null && a.coupe, "phase A : le flux s'est arrêté SANS événement `fin` (coupure reproduite)");
  // ── LA CAUSE DU DÉFAUT, REPRODUITE POUR DE VRAI ──────────────────────────
  const tues = tuerLeService();
  noter(tues > 0, `service tué (${tues} processus, SIGKILL comme un déploiement)`);
  const retour = await attendreServeur(30_000);
  noter(retour !== null, retour === null
    ? "le service n'est pas revenu dans les 30 s"
    : `service revenu en ${retour} ms (systemd, RestartSec=2)`);
  if (partiel.length === 0) {
    partiel = "L'utilisateur demande : " + QUESTION;
  }
} else if (a.fin) {
  noter(true, `phase A : fin naturelle (raison « ${a.fin.raison} », ${a.ms} ms)`);
}

// ── PHASE B : LA REPRISE — la réponse CONTINUE au lieu d'être incomplète ────
const b = await ouvrirFlux({ deja: partiel, limiteMs: 240_000 });
noter(b.status === 200, `reprise : flux rouvert avec ${partiel.length} caractères déjà produits`);
noter(b.erreur === null, b.erreur ? `reprise refusée : ${b.erreur}` : "reprise acceptée par le serveur");
noter(b.fin !== null, b.fin ? `reprise terminée (raison « ${b.fin.raison} »)` : "la reprise n'a pas rendu de `fin`");
// Une SUITE non vide suffit : la longueur de la reprise est celle qui manquait,
// pas une quantité à atteindre. Mesuré : la réponse « 1, 2, … 17, 1 » coupée en
// plein « 18 » se reprend par « 8, 19, 20 » — neuf caractères, et c'est la bonne
// réponse. Exiger « au moins dix » ferait échouer une reprise EXACTE.
noter(b.texte.length > 0, `reprise : ${b.texte.length} caractères de SUITE reçus`);
console.log(`        suite  : ${JSON.stringify(b.texte.slice(0, 80))}`);

// ── CONTINUER OU RÉGÉNÉRER : LES DEUX ARRIVENT, ET LES DEUX SONT TRAITÉS ────
// Mesuré le même jour, sur la même question : une fois la suite reprend en plein
// mot (« …17, 1 » puis « 8, 19, 20 »), une fois le modèle RÉGÉNÈRE toute la
// réponse (« 1, 2, 3, … 20 »). Le protocole ne peut pas l'imposer — c'est le
// RACCORD DE LA VUE qui rend les deux cas justes (`raccord.ts`, testé par
// `raccord.test.mjs`). On le RAPPORTE ici au lieu d'en faire un échec : ce serait
// exiger d'un modèle une discipline qu'on ne lui a pas donnée.
const tete = partiel.slice(0, 40).trim();
if (tete.length >= 20) {
  const recommence = b.texte.trim().startsWith(tete);
  console.log(recommence
    ? "  note  la suite RÉGÉNÈRE la réponse — le raccord de la vue la fait remplacer le texte affiché"
    : "  ok    la suite CONTINUE la réponse (aucun recouvrement à traiter)");
}

// Et le défaut refusé ne doit apparaître nulle part dans ce que le serveur envoie.
const texteComplet = partiel + b.texte;
noter(!texteComplet.includes("flux interrompu"), "aucun « flux interrompu » dans le flux servi");

const echecs = faits.filter((f) => !f.ok);
console.log(`── ${echecs.length === 0 ? "PASS" : `FAIL (${echecs.length})`} — reprise de flux`);
process.exit(echecs.length === 0 ? 0 : 1);
