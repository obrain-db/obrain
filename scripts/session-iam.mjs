// SESSION IAM POUR LES BANCS — un serveur armé ne répond plus sans identité.
//
// Défaut corrigé : les trois bancs de vérification (`chat-reprise-check`,
// `requetes-latence-check`, `chat-reprise-ui-check`) parlaient à un serveur
// OUVERT. Depuis que la chaîne d'accès est armée sur :8432 (2026-09-13), un
// appel sans identité reçoit **401**, et les bancs qui prouvent les exigences du
// chat ne pouvaient donc plus rien prouver — le filet lâchait exactement quand la
// configuration changeait.
//
// Ce module ouvre une session par le MÊME chemin que le navigateur
// (`POST /api/auth/login`, formulaire, cookie), et rend le cookie à poser sur les
// requêtes suivantes. Sur un serveur NON armé (200 sans identité), il ne fait
// rien et le dit : un banc ne doit pas exiger d'authentification là où il n'y en
// a pas.
//
// Usage :
//   const session = await ouvrirSession(BASE);
//   ... headers: session.enTetes()   // { Cookie: "…" } ou {}
//
// Les identifiants viennent, dans l'ordre : des options, de `OBRAIN_IAM_USER` /
// `OBRAIN_IAM_PASSWORD`, ou du fichier d'identifiants du harness. Le mot de passe
// n'est JAMAIS affiché.

import { readFileSync, existsSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const FICHIER_DEFAUT = join(
  homedir(),
  ".local/share/obrain-harness/data/iam.obrain.identifiants.txt",
);

/** Lit `utilisateur : …` et `mot de passe : …` d'un fichier d'identifiants. */
export function lireIdentifiants(chemin = FICHIER_DEFAUT) {
  if (!existsSync(chemin)) return null;
  const texte = readFileSync(chemin, "utf8");
  const champ = (motif) => {
    const m = new RegExp(`^ *${motif} *: *(.*)$`, "m").exec(texte);
    return m ? m[1].trim() : null;
  };
  const utilisateur = champ("utilisateur");
  const motDePasse = champ("mot de passe");
  return utilisateur && motDePasse ? { utilisateur, motDePasse } : null;
}

/**
 * Ouvre une session si le serveur en exige une.
 *
 * Rend `{ arme, cookie, enTetes() }` — `enTetes()` rend `{}` sur un serveur
 * ouvert, ou `{ Cookie }` sur un serveur armé.
 */
export async function ouvrirSession(base, options = {}) {
  const sonde = await fetch(new URL("/api/stores", base), {
    signal: AbortSignal.timeout(options.delaiMs ?? 15_000),
  }).catch((e) => {
    throw new Error(`serveur injoignable (${base}) : ${e.message}`);
  });

  if (sonde.status === 200) {
    return { arme: false, cookie: null, enTetes: () => ({}), note: "serveur ouvert (IAM non exigé)" };
  }
  if (sonde.status !== 401) {
    throw new Error(`sonde d'identité inattendue : HTTP ${sonde.status}`);
  }

  const identifiants =
    (options.utilisateur && options.motDePasse
      ? { utilisateur: options.utilisateur, motDePasse: options.motDePasse }
      : null) ??
    (process.env.OBRAIN_IAM_USER && process.env.OBRAIN_IAM_PASSWORD
      ? { utilisateur: process.env.OBRAIN_IAM_USER, motDePasse: process.env.OBRAIN_IAM_PASSWORD }
      : null) ??
    lireIdentifiants(options.identifiants);

  if (!identifiants) {
    throw new Error(
      "serveur ARMÉ et aucun identifiant : passez --user/--password, OBRAIN_IAM_USER/" +
        "OBRAIN_IAM_PASSWORD, ou le fichier d'identifiants du harness",
    );
  }

  const corps = new URLSearchParams({
    username: identifiants.utilisateur,
    password: identifiants.motDePasse,
  });
  const reponse = await fetch(new URL("/api/auth/login", base), {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: corps,
    redirect: "manual",
    signal: AbortSignal.timeout(options.delaiMs ?? 15_000),
  });

  // `303` (redirection après connexion) et `200` sont les deux issues acceptées
  // par le script d'acceptation du projet.
  if (reponse.status !== 200 && reponse.status !== 303) {
    throw new Error(`connexion refusée : HTTP ${reponse.status} (identifiants ou espace)`);
  }
  const brut = reponse.headers.getSetCookie?.() ?? [];
  const cookie = brut.map((c) => c.split(";")[0]).join("; ");
  if (!cookie) throw new Error("connexion acceptée mais aucun cookie de session rendu");

  return {
    arme: true,
    cookie,
    enTetes: () => ({ Cookie: cookie }),
    note: `serveur armé — session ouverte pour « ${identifiants.utilisateur} »`,
  };
}
