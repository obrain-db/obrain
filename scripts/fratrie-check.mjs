#!/usr/bin/env node
// FRATRIE — ce qui entoure un élément sélectionné : frères, sœurs, co-fichiers.
//
// DEMANDE VÉRIFIÉE : « qu'on voie bien les frères et les sœurs en cas de
// sélection d'un élément, pour garantir l'affichage de l'immensité
// d'information. »
//
// Cinq contrôles, chacun contre le service VIVANT (lecture seule : navigation
// et clics), et chacun capable d'échouer pour une raison PRÉCISE :
//
//  1. UNICITÉ DE LA RÈGLE DE PARENTÉ. La parenté se dérivait en deux
//     exemplaires incompatibles. On teste la fonction RÉELLE du produit
//     (`window.__fratrie.parentDe`, installée par `src/graph/fratrie.ts`) sur
//     des qn réels, contre les deux anciennes règles, et l'on exige que les
//     anciennes divergent ENTRE ELLES exactement sur l'ensemble `__file__` et
//     nulle part ailleurs — c'est-à-dire que la règle unique reproduit celle
//     qui filtrait le marqueur.
//     On exige AUSSI la preuve de bout en bout : une puce de filtre de la vue
//     Cosmos ALLUME réellement l'effectif qu'elle annonce (mesuré sur la
//     variable d'emphase réelle, `window.__etages.filtre`). Avant la règle
//     unique, cette puce ne pouvait structurellement rien allumer.
//  2. SÉLECTION → FRATRIE NON VIDE, TOTAL ANNONCÉ. Face Graphe : on sélectionne
//     un nœud et on exige que la barre de facettes annonce des frères et des
//     sœurs, avec leur effectif.
//  3. COHÉRENCE AFFICHÉS / INDEX, SANS TRONCATURE MUETTE. Le nombre de frères
//     affichés doit valoir min(total, plafond), le total doit être écrit, et
//     au-delà du plafond un dépliage doit révéler EXACTEMENT le total.
//  4. CLIQUER UN FRÈRE CHANGE LA CIBLE (état ET rendu), et la fratrie reste
//     ouverte sur le nouveau nœud — le piège étant que `ancrerAtom` vide la
//     sélection.
//  5. L'INDEX EST LOCAL : sa construction n'émet AUCUNE requête. On le prouve
//     en comptant les entrées `performance`, puis on chronomètre la
//     construction aux trois tailles du dépôt (160 / 8 000 / 200 000) sur des
//     qn réels — et l'on montre, dans le même moteur, que l'ancien
//     `Math.min(...a)` levait bien `RangeError` à 200 000.
//
// Usage : node scripts/fratrie-check.mjs [base] [chrome-cdp]
import { readFileSync } from "node:fs";
import { homedir } from "node:os";

const BASE = process.argv[2] || "http://192.168.1.103:8432";
const CDP = process.argv[3] || process.env.CDP_URL || "http://127.0.0.1:9248";
const STORE_DEMANDE = process.env.FRATRIE_STORE || "";
const CENTRE_DEMANDE = process.env.FRATRIE_CENTRE || "";
const PLAFOND = 14; // doit rester égal à PLAFOND_AFFICHE (FacetBar.tsx)

/**
 * ── LA CIBLE EST CHOISIE SUR MESURE, JAMAIS SUPPOSÉE ────────────────────────
 *
 * Défaut constaté (rapport du propriétaire, 2026-09-13) : ce contrôle exigeait
 * le store « Linux e1000 », qui n'existe PAS sur toutes les instances (absent
 * des presets de l'instance isolée 8433). L'application retombait alors sur son
 * store par défaut — un store « runtime », donc la face MONDE —, la barre de
 * facettes ne pouvait pas se monter, et le contrôle échouait pour une raison
 * qui ne devait rien au produit. Un contrôle ne suppose pas sa donnée : il la
 * cherche, et il DIT ce qu'il a trouvé.
 *
 * Critère : un store servi et un point d'entrée dont le sous-graphe contient un
 * groupe de fratrie d'au moins 15 nœuds — pour que le plafond d'affichage de 14
 * soit réellement exercé (sinon le contrôle de troncature ne teste rien).
 *
 * La règle de parenté est recopiée ici À DESSEIN pour ce CHOIX de cible : c'est
 * une heuristique de ciblage, jamais une assertion. Les assertions, elles,
 * portent sur le produit (`window.__fratrie.parentDe`) et sur le panneau affiché.
 */
const seg = (q) => String(q || "").split(/[./]/).filter((x) => x.length);
const court = (s) => (s.length > 22 ? s.slice(0, 22) : s);
const net = (s) => (s || "").replace(/^_+$/, "?") || "?";
const baseChemin = (p) => {
  if (!p) return undefined;
  const c = String(p).replace(/[/\\]+$/, "");
  const i = Math.max(c.lastIndexOf("/"), c.lastIndexOf("\\"));
  return i >= 0 ? c.slice(i + 1) : c;
};
const dossierChemin = (p) => {
  if (!p) return undefined;
  const c = String(p).replace(/[/\\]+$/, "");
  const i = Math.max(c.lastIndexOf("/"), c.lastIndexOf("\\"));
  if (i <= 0) return undefined;
  return baseChemin(c.slice(0, i));
};
const parentDeLocal = (q, fp) => {
  const d = dossierChemin(fp);
  if (d) return court(net(d));
  const p = seg(q);
  const u = p.filter((x) => x !== "__file__");
  const s = u.length ? u : p;
  if (!s.length) return "?";
  return court(net(s.length >= 2 ? s[s.length - 2] : s[0]));
};

async function choisirCible() {
  const sig = await api("/api/viz/signatures");
  const labels = (sig.stores || []).map((s) => s.label);
  // ── DEUX CRITÈRES QUE LE CONTRÔLE DOIT RESPECTER, SOUS PEINE DE SE MENTIR ─
  // 1. le store doit être NON-runtime : `App` route les stores « runtime/self/
  //    engram » vers la face MONDE, où `FacetBar` n'existe pas (mesuré) ;
  // 2. le point d'entrée doit être CELUI QUE L'APPLICATION CHOISIT ELLE-MÊME
  //    (`App.defaultsFor`) : chercher un autre sous-graphe pour exercer le
  //    plafond de 14 serait tester un écran que l'utilisateur ne voit pas.
  //    La règle de `App` est recopiée ici, avec sa source.
  const runtimeLike = (l) => {
    const x = String(l).toLowerCase();
    return x.includes("runtime") || x.includes("self") || x.includes("engram");
  };
  const defaultsFor = (label) => {
    const l = String(label).toLowerCase();
    if (l.includes("complet")) return "tcp_v4_rcv";
    if (l.includes("linux")) return "e1000_probe";
    return "substitute_in_operator";
  };
  const preferences = [STORE_DEMANDE, "Linux e1000", "Obrain (rust)",
    ...labels.filter((l) => !runtimeLike(l)), ...labels].filter(Boolean);
  const essais = [];
  let meilleur = null;
  // ── L'ANCRE EST UN NOM NU : LA RÉSOUDRE EN qn, COMME LE FAIT L'APPLICATION ─
  // AJOUTÉ LE 2026-09-15. `defaultsFor` ancre sur un NOM NU (`e1000_probe`,
  // `tcp_v4_rcv`). La lignée servie résout en STRICT (id ou qn exact) et répond
  // 400 « centre introuvable » ; celle du tronc a un repli par suffixe. Les
  // sondes ci-dessous, qui interrogeaient le nom nu, échouaient donc sur le
  // service 8432 — et le contrôle, faute de cible, mesurait le store par défaut.
  // L'application résout désormais le nom en qn (`resoudreAncres`, api.ts) : la
  // sonde fait le MÊME geste, sinon elle testerait un chemin que le produit
  // n'emprunte plus.
  const enQn = async (label, nom) => {
    if (/^\d+$/.test(nom) || nom.includes(".")) return nom;
    const r = await api(`/api/functions?store=${encodeURIComponent(label)}&q=${encodeURIComponent(nom)}&limit=5`).catch(() => null);
    const hit = (r?.items || []).find((h) => h.name === nom) || (r?.items || [])[0];
    return hit?.qn || nom;
  };
  for (const label of [...new Set(preferences)]) {
    if (!labels.includes(label)) { essais.push(`${label} : absent de cette instance`); continue; }
    if (runtimeLike(label)) { essais.push(`${label} : store « runtime » → App ouvre la face MONDE, pas la face Graphe`); continue; }
    const mods = await api(`/api/viz/modules?store=${encodeURIComponent(label)}&top=6`).catch(() => null);
    const centres = [CENTRE_DEMANDE, defaultsFor(label), ...(mods?.modules || []).map((m) => m.name)].filter(Boolean);
    for (const centre of [...new Set(centres)]) {
      const vise = await enQn(label, centre);
      const g = await api(`/api/viz/subgraph?${new URLSearchParams({
        store: label, center: vise, hops: "1", limit: "160", overlay: "",
      })}`).catch(() => null);
      const nodes = (g?.nodes || []).map((n) => ({ qn: String(n.qn || n.id), fp: n.file_path }));
      if (nodes.length < 5) { if (centre === centres[0]) essais.push(`${label}/${centre}${vise !== centre ? ` (résolu en ${vise.slice(0, 50)})` : ""} : ${nodes.length} nœud(s) — entrée non exploitable`); continue; }
      const par = new Map();
      for (const n of nodes) {
        const p = parentDeLocal(n.qn, n.fp);
        if (!par.has(p)) par.set(p, []);
        par.get(p).push(n.qn);
      }
      const [groupe, membres] = [...par.entries()].sort((a, b) => b[1].length - a[1].length)[0] || ["?", []];
      const cand = { store: label, centre, noeuds: nodes.length, groupe, effectif: membres.length, essais, defaut: centre === centres[0] };
      // Le plafond de 14 doit être DÉPASSÉ pour que la troncature annoncée soit testée.
      if (membres.length - 1 > PLAFOND) return cand;
      if (centre === centres[0]) essais.push(`${label} (entrée par défaut « ${centre} ») : ${membres.length - 1} frère(s) — le plafond de ${PLAFOND} n'est pas exercé`);
      if (!meilleur || membres.length > meilleur.effectif) meilleur = cand;
    }
  }
  return meilleur || { store: null, essais };
}

// ── IDENTITÉ : LE SERVICE PEUT ÊTRE ARMÉ (IAM) ──────────────────────────────
// Depuis le 2026-09-13, `8432` peut exiger une identité sur CHAQUE requête
// (`Authorization: Bearer <jeton>`, ou le cookie `obrain_session`) : mesuré, un
// contrôle qui ne la présente pas reçoit 401 et ne mesure plus rien. Les
// identifiants locaux sont ceux que le service a engendrés (`obrain viz serve
// --iam`), dans le fichier documenté ci-dessous — surchargeables par
// l'environnement pour un autre déploiement.
const IDENTIFIANTS = process.env.OBRAIN_IAM_IDENTIFIANTS
  || `${homedir()}/.local/share/obrain-harness/data/iam.obrain.identifiants.txt`;

/** Jeton à présenter : celui de l'environnement, sinon une session ouverte. */
let JETON = process.env.OBRAIN_IAM_TOKEN || "";
let AUTH = "non tentée";
async function sAuthentifier() {
  if (JETON) { AUTH = "jeton d'environnement (OBRAIN_IAM_TOKEN)"; return; }
  // ── ARMÉ OU PAS : ON LE DEMANDE, ON NE LE SUPPOSE PAS ───────────────────
  // Le service de développement a été armé puis désarmé dans la même journée
  // (IAM compilé, `OBRAIN_IAM=1` posé ou non). Un contrôle qui suppose l'un des
  // deux états échoue pour une raison qui ne doit rien au produit : on sonde
  // donc `/api/viz/signatures` sans identité et l'on décide sur la RÉPONSE.
  const sonde = await fetch(BASE + "/api/viz/signatures", { redirect: "manual" }).catch(() => null);
  const code = sonde?.status ?? 0;
  if (code === 200) { AUTH = "aucune identité requise (IAM non armé) — vérifié par sonde"; return; }
  if (code !== 401) {
    AUTH = `service injoignable ou inattendu (HTTP ${code || "aucune réponse"}) sur ${BASE}`;
    return;
  }
  let brut = null;
  try { brut = readFileSync(IDENTIFIANTS, "utf8"); } catch { /* pas d'identifiants locaux */ }
  if (!brut) { AUTH = "IAM ARMÉ et aucun identifiant local — voir docs/IAM-BRANCHE-2026-09-13.md"; return; }
  const motDePasse = (brut.match(/mot de passe\s*:\s*(\S+)/) || [])[1];
  const utilisateur = (brut.match(/utilisateur\s*:\s*(\S+)/) || [])[1] || "admin";
  if (!motDePasse) { AUTH = "fichier d'identifiants illisible"; return; }
  const r = await fetch(BASE + "/api/auth/login", {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({ username: utilisateur, password: motDePasse }).toString(),
    redirect: "manual",
  }).catch(() => null);
  const setCookie = r?.headers?.get("set-cookie") || "";
  JETON = (setCookie.match(/obrain_session=([^;]+)/) || [])[1] || "";
  AUTH = JETON ? `IAM armé — session ouverte (utilisateur ${utilisateur}, cookie obrain_session)`
    : `IAM armé mais connexion refusée (HTTP ${r?.status ?? "?"})`;
}

const j = async (p, init) => {
  const r = await fetch(CDP + p, init);
  const t = await r.text();
  try { return JSON.parse(t); } catch { return t; }
};
const api = async (p) => {
  const r = await fetch(BASE + p, { headers: JETON ? { authorization: `Bearer ${JETON}` } : {} });
  // ── UN 401 DOIT DIRE POURQUOI, PAS SEULEMENT QUOI ────────────────────────
  // Constaté le 2026-09-16 en visant le service ARMÉ : le script mourait sur
  // `HTTP 401 sur /api/viz/signatures` avec une pile d'appels, sans jamais dire
  // que l'identité manquait ni pourquoi — alors que `AUTH` sait toujours laquelle
  // des issues a été prise (jeton d'environnement, IAM non armé, identifiants
  // locaux absents, connexion refusée…). Un contrôle qui échoue sans dire sa
  // cause se lit comme une panne du produit, et c'est ce qu'il ne doit pas faire.
  if (!r.ok) throw new Error(`HTTP ${r.status} sur ${p} · identité : ${AUTH}`);
  return r.json();
};

// ── Les deux ANCIENNES règles, recopiées ici À DESSEIN ──────────────────────
// Un contrôle qui réutiliserait la nouvelle fonction pour juger l'ancienne ne
// mesurerait rien : il faut les deux règles telles qu'elles étaient écrites,
// TRONCATURE DE 22 CARACTÈRES INCLUSE (les deux sites la portaient), sinon on
// comparerait deux choses différentes.
const ANCIENNE_COSMOSWEAVE = `(qn) => {
  const p = String(qn).split(/[./]/).filter(Boolean);
  const parent = p.length >= 2 ? p[p.length - 2] : (p[0] || qn);
  return parent.length > 22 ? parent.slice(0, 22) : parent;
}`; // CosmosWeave.tsx:2446-2451 — aucune mention de __file__
const ANCIENNE_ETIQUETTE = `(qn) => {
  const src = String(qn).split(/[./]/).filter((x) => x && x !== "__file__");
  const brut = src.length >= 2 ? src[src.length - 2] : (src[0] || "?");
  const t = (brut || "?").replace(/^_+$/, "?");
  return t.length > 22 ? t.slice(0, 22) : t;
}`; // CosmosWeave.tsx:662-666 — filtre __file__ PUIS avant-dernier

/**
 * Un onglet CDP, avec ses aides (texte, clic, touche, évaluation).
 *
 * Le COOKIE est posé AVANT la navigation : sans lui, le service armé répond
 * `303 → /login` et l'onglet afficherait un formulaire de connexion — un
 * contrôle qui mesurerait une page de connexion en croyant mesurer le
 * dashboard.
 */
async function onglet(url) {
  const tab = await j("/json/new?about:blank", { method: "PUT" });
  if (!tab?.webSocketDebuggerUrl) return null;
  const ws = new WebSocket(tab.webSocketDebuggerUrl);
  let id = 0;
  const pending = new Map();
  const errors = [];
  const send = (method, params = {}) =>
    new Promise((res) => { const i = ++id; pending.set(i, res); ws.send(JSON.stringify({ id: i, method, params })); });
  ws.addEventListener("message", (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pending.has(m.id)) { pending.get(m.id)(m.result); pending.delete(m.id); return; }
    if (m.method === "Runtime.exceptionThrown") errors.push(m.params.exceptionDetails.text);
    if (m.method === "Log.entryAdded" && m.params.entry.level === "error") errors.push(m.params.entry.text);
  });
  await new Promise((r) => ws.addEventListener("open", r, { once: true }));
  await send("Runtime.enable"); await send("Log.enable"); await send("Page.enable");
  if (JETON) {
    await send("Network.enable");
    const hote = new URL(BASE).hostname;
    await send("Network.setCookie", {
      name: "obrain_session", value: JETON, domain: hote, path: "/", httpOnly: true, sameSite: "Strict",
    });
  }
  await send("Page.navigate", { url });
  const evalIn = async (expression) =>
    (await send("Runtime.evaluate", { returnByValue: true, awaitPromise: true, expression }))?.result?.value;
  const texte = () => evalIn("document.body.innerText");
  const touche = async (k) => {
    await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key: k, code: k });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key: k, code: k });
  };
  const fermer = async () => { await j("/json/close/" + tab.id); ws.close(); };
  return { evalIn, texte, touche, send, errors, fermer };
}
const dormir = (ms) => new Promise((r) => setTimeout(r, ms));

let bad = 0;
const lignes = [];
const P = (ok, texte) => { if (!ok) bad++; lignes.push(`${ok ? "✓" : "✗"} ${texte}`); };

// L'identité d'abord : tout ce qui suit (API et onglets) en dépend.
await sAuthentifier();
console.log("identité : " + AUTH + "\n");
// La CIBLE ensuite : elle est cherchée sur l'instance interrogée, jamais supposée.
const CIBLE = await choisirCible();

// ── UN CONTRÔLE QUI N'A PAS DE CIBLE NE MESURE RIEN ─────────────────────────
// AJOUTÉ LE 2026-09-15. Avant, `CIBLE.store` valait `null` et le script
// CONTINUAIT : l'URL devenait `?store=null`, l'application retombait sur le
// store par défaut (un store « runtime », donc la face MONDE sans `FacetBar`),
// et les échecs qui suivaient accusaient la FRATRIE alors qu'ils accusaient une
// cible manquante. Les essais étaient collectés… et jamais imprimés. On
// s'arrête donc ici, en disant POURQUOI — et la cause se lit dans les essais.
if (!CIBLE.store) {
  console.error("FRATRIE : aucune cible exploitable sur cette instance — le contrôle NE MESURE PAS.");
  console.error("  essais de sélection :");
  for (const e of (CIBLE.essais || [])) console.error(`    · ${e}`);
  console.error("  forcer la cible si elle est connue : FRATRIE_STORE=… FRATRIE_CENTRE=…");
  process.exit(3);
}

// ═══════════════════════════════════════════════════════════════════════════
// 1. UNICITÉ DE LA RÈGLE DE PARENTÉ
// ═══════════════════════════════════════════════════════════════════════════
// Les qn viennent du SERVICE, pas d'un fichier de test : ce sont les formes
// réelles (chemins pointés, marqueurs `__file__`, identifiants numériques).
async function qnReels() {
  const out = new Set();
  const paires = new Map(); // qn → file_path RÉEL (pour séparer l'écart dû à la donnée)
  const sig = await api("/api/viz/signatures");
  for (const s of (sig.stores || []).slice(0, 4)) {
    const q = await api(`/api/query?${new URLSearchParams({
      store: s.label, query: "MATCH (n) RETURN n.qualified_name AS qn LIMIT 300", language: "gql",
    })}`).catch(() => null);
    for (const row of (q?.rows || [])) if (typeof row[0] === "string" && row[0]) out.add(row[0]);
    // Les qn marqués `__file__` ne sortent PAS de la requête ci-dessus (elle rend
    // les fonctions) : ils viennent des sous-graphes, où le marqueur vit — et
    // c'est là aussi que le `file_path` réel est disponible.
    const mods = await api(`/api/viz/modules?store=${encodeURIComponent(s.label)}&top=4`).catch(() => null);
    for (const m of (mods?.modules || [])) {
      const g = await api(`/api/viz/subgraph?${new URLSearchParams({
        store: s.label, center: m.name, hops: "1", limit: "60", overlay: "",
      })}`).catch(() => null);
      for (const n of (g?.nodes || [])) {
        if (!n.qn) continue;
        out.add(String(n.qn));
        if (n.file_path) paires.set(String(n.qn), String(n.file_path));
      }
    }
  }
  return { qns: [...out], paires: [...paires.entries()].map(([qn, fp]) => ({ qn, fp })).slice(0, 600) };
}
/** Récolté UNE fois : deux appels doubleraient le temps du contrôle. */
let RECOLTE = null;
const recolte = async () => (RECOLTE ||= await qnReels());
const qnsReels = async () => (await recolte()).qns;

let unicite = null;
{
  const qns = await qnsReels();
  const paires = (await recolte()).paires;
  const t = await onglet(BASE + "/");
  if (!t) { P(false, "CDP : onglet refusé pour le contrôle d'unicité"); }
  else {
    try {
      // ── ON ATTEND L'INSTRUMENT, ON NE DORT PAS ───────────────────────────
      // Défaut constaté (rapport du propriétaire, 2026-09-13) : ce contrôle
      // dormait 4,5 s puis concluait « window.__fratrie.parentDe absent » — un
      // FAUX NÉGATIF. `window.__fratrie` est installé par le module, donc dès
      // que le bundle a été évalué ; mais l'application met 7 à 10 s à se
      // monter sur cette machine (mesuré : `__pickSweep`, posé par la scène
      // Cosmos, manque encore bien après). Une attente à durée fixe est une
      // course : on attend donc LA CHOSE MESURÉE, avec une borne, et l'on
      // rapporte le délai observé.
      const t0 = Date.now();
      let pret = false;
      for (let k = 0; k < 30 && !pret; k++) {
        pret = !!(await t.evalIn("!!(window.__fratrie && typeof window.__fratrie.parentDe === 'function')"));
        if (!pret) await dormir(1000);
      }
      unicite = { attenteMs: Date.now() - t0 };
      if (!pret) { P(false, `window.__fratrie absent après ${unicite.attenteMs} ms d'attente (30 sondes)`); }
      else {
      const r = await t.evalIn(`(() => {
        const F = window.__fratrie;
        const ancienne = ${ANCIENNE_COSMOSWEAVE};
        const etiquette = ${ANCIENNE_ETIQUETTE};
        const qns = ${JSON.stringify(qns)};
        let divAnciennes = 0, divNouvelle = 0, divNouvelleHorsFile = 0;
        const avecFile = qns.filter((q) => q.split(/[./]/).includes("__file__"));
        const sansFile = qns.filter((q) => !q.split(/[./]/).includes("__file__"));
        const ex = [];
        for (const q of qns) {
          const a = ancienne(q), b = etiquette(q), n = F.parentDe(q);
          if (a !== b) divAnciennes++;
          if (b !== n) divNouvelle++;
          if (n !== a) {
            if (!q.split(/[./]/).includes("__file__")) divNouvelleHorsFile++;
            if (ex.length < 5) ex.push({ qn: q, ancienne: a, etiquette: b, nouvelle: n });
          }
        }
        // L'ANCIENNE RÈGLE DU FILTRE DOIT ÊTRE REPRODUITE À L'IDENTIQUE : c'est
        // elle qui portait le bon comportement (filtrer le marqueur __file__).
        const identiqueEtiquette = divNouvelle === 0;

        // ── L'ÉCART CAUSÉ PAR LA DONNÉE file_path : ATTENDU, ET EXPLIQUÉ ──
        // La règle canonique reçoit en plus le dossier RÉEL du fichier. Quand ce
        // dossier diffère du segment porteur du qn, l'écart avec l'ancienne règle
        // est VOULU (c'est la correction) — mais il ne doit pas être compté comme
        // un défaut, ni passer sans preuve : on exige que CHAQUE écart soit
        // expliqué par le dossier réel, et l'on compte ceux qui ne le sont pas.
        const dossier = (p) => {
          if (!p) return null;
          const c = String(p).replace(/[/\\\\]+$/, '');
          const i = Math.max(c.lastIndexOf('/'), c.lastIndexOf('\\\\'));
          if (i <= 0) return null;
          const d = c.slice(0, i);
          const j = Math.max(d.lastIndexOf('/'), d.lastIndexOf('\\\\'));
          const b = j >= 0 ? d.slice(j + 1) : d;
          return b.length > 22 ? b.slice(0, 22) : b;
        };
        const paires = ${JSON.stringify(paires)};
        let divChemin = 0, divCheminVsAncienne = 0, cheminExplique = 0;
        const exChemin = [];
        for (const pr of paires) {
          const sansChemin = F.parentDe(pr.qn);
          const avecChemin = F.parentDe(pr.qn, pr.fp);
          if (avecChemin !== sansChemin) divChemin++;
          if (avecChemin !== ancienne(pr.qn)) {
            divCheminVsAncienne++;
            const d = dossier(pr.fp);
            if (d && d === avecChemin) cheminExplique++;
            else if (exChemin.length < 4) exChemin.push({ qn: pr.qn, fp: pr.fp, ancienne: ancienne(pr.qn), avecChemin, dossier: d });
          }
        }
        return {
          n: qns.length, avecFile: avecFile.length, sansFile: sansFile.length,
          divAnciennes, divNouvelle, divNouvelleHorsFile, identiqueEtiquette, ex,
          paires: paires.length, divChemin, divCheminVsAncienne, cheminExplique, exChemin,
        };
      })()`);
      unicite = { ...unicite, ...r };
      if (r?.err) P(false, `règle de parenté : ${r.err}`);
      else {
        P(r.identiqueEtiquette,
          `parentDe(qn) EST la règle filtrant « __file__ » : ${r.n} qn RÉELS, écart ${r.divNouvelle} ` +
          `(dont ${r.avecFile} portant __file__)`);
        P(r.divNouvelleHorsFile === 0,
          `aucun écart hors marqueur sur la seule règle de nom : ${r.sansFile} qn sans __file__ → ` +
          `${r.divNouvelleHorsFile} divergence(s)`);
        P(r.divAnciennes === r.avecFile && r.avecFile > 0,
          `les DEUX anciennes règles divergeaient sur ${r.divAnciennes} qn = exactement les ` +
          `${r.avecFile} portant __file__ (c'est le défaut réparé)`);
        // L'écart dû à la DONNÉE est attendu — et il doit être INTÉGRALEMENT
        // expliqué par le dossier réel du fichier. Un seul écart inexpliqué
        // serait un vrai défaut de la règle.
        P(r.paires > 0 && r.cheminExplique === r.divCheminVsAncienne,
          `écart ATTENDU dû à file_path : ${r.divCheminVsAncienne} nœuds sur ${r.paires} paires réelles ` +
          `où le dossier réel diffère du segment porteur — dont ${r.cheminExplique} EXPLIQUÉS par le dossier ` +
          `réel (${r.divChemin} changent même sans l'ancienne règle)` +
          (r.exChemin?.length ? ` · ${r.exChemin.length} INEXPLIQUÉ(S)` : " · aucun inexpliqué"));
        if (r.ex?.length) {
          lignes.push(`   exemple (règle de nom) : ${r.ex[0].qn.slice(-60)}`);
          lignes.push(`      ancienne « ${r.ex[0].ancienne} » / étiquette « ${r.ex[0].etiquette} »` +
            ` → parentDe « ${r.ex[0].nouvelle} »`);
        }
        if (r.exChemin?.length) {
          for (const e of r.exChemin.slice(0, 2)) {
            lignes.push(`   INEXPLIQUÉ : ${e.qn.slice(-50)} | file_path ${e.fp} | dossier lu « ${e.dossier} »` +
              ` | parentDe(avec chemin) « ${e.avecChemin} » | ancienne « ${e.ancienne} »`);
          }
        }
      }
      }
    } finally { await t.fermer(); }
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// 5b. LE PIÈGE `Math.min(...a)` : REPRODUIT DANS LE MÊME MOTEUR
// ═══════════════════════════════════════════════════════════════════════════
let pile = null;
{
  const t = await onglet(BASE + "/");
  if (t) {
    try {
      await dormir(3500);
      pile = await t.evalIn(`(() => {
        const n = 200000;
        const a = new Float64Array(n);
        for (let i = 0; i < n; i++) a[i] = (i % 977) / 977;
        let etale = null;
        try { Math.min(...a); etale = "aucune erreur"; }
        catch (e) { etale = String(e && e.message || e).slice(0, 60); }
        // UN SEUL PARCOURS : la correction.
        let min = Infinity, max = -Infinity; const vues = new Set();
        const t0 = performance.now();
        for (let i = 0; i < n; i++) { const v = a[i]; if (v < min) min = v; if (v > max) max = v; vues.add(v); }
        const ms = performance.now() - t0;
        return { n, etale, min, max, distinctes: vues.size, ms: +ms.toFixed(2) };
      })()`);
      if (pile) {
        P(/Maximum call stack/.test(pile.etale || ""),
          `Math.min(...a) sur ${pile.n} nœuds lève « ${pile.etale} » — un seul parcours rend ` +
          `min ${pile.min.toFixed(3)} / max ${pile.max.toFixed(3)} / ${pile.distinctes} distinctes en ${pile.ms} ms`);
      }
    } finally { await t.fermer(); }
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// 1b + 2 + 3 + 4. LA FRATRIE, À L'ÉCRAN
// ═══════════════════════════════════════════════════════════════════════════
let ecran = null;
{
  // ── LA ROUTE VERS LA FACE GRAPHE, ET POURQUOI ELLE EST CELLE-LÀ ──────────
  //  · la face par défaut EST « graphe » : on ne clique donc PAS « 🔺 Graphe »
  //    (mesuré : depuis une autre face, ce bouton fait entrer dans le COSMOS,
  //    à l'étage 2 — « les dimensions sont des échelles, pas des pages » — et
  //    `FacetBar` n'existe pas dans le Cosmos) ;
  //  · `morph` vaut `true` par défaut : il faut le COUPER, sinon c'est
  //    `StageContinu` qui est monté, pas `Graph3D` — donc pas de sélection ;
  //  · `portee` unique : le pont d'état d'interface restaure sinon une vue
  //    distante (`/ui/state/<portee>`), et un autre contrôle qui aurait laissé
  //    « cosmos » dans le document ferait échouer celui-ci pour une raison qui
  //    ne doit rien au produit.
  const url = `${BASE}/?store=${encodeURIComponent(CIBLE.store)}&portee=fratrie-check-${process.pid}`;
  const t = await onglet(url);
  if (!t) { P(false, "CDP : onglet refusé pour les contrôles d'écran"); }
  else {
    try {
      // ── ON ATTEND LA SCÈNE MONTÉE, PUIS LE GRAPHE CHARGÉ ─────────────────
      // Deux courses trouvées en rejouant ce contrôle contre l'instance isolée
      // 8433 : (1) une durée fixe de 9 s ne suffit pas toujours pour que
      // l'application se monte ; (2) la touche ←/→ est un geste CLAVIER, et
      // `Graph3D` l'ignore tant que `c.pos` est vide — un `ArrowRight` envoyé
      // pendant le chargement ne sélectionne donc RIEN, sans erreur. On attend
      // donc des ÉTATS OBSERVABLES : la scène, puis le nombre de nœuds chargés
      // (la ligne de statistiques), puis l'apparition de la barre de facettes.
      let monte = false;
      for (let k = 0; k < 30 && !monte; k++) {
        monte = !!(await t.evalIn("!!document.querySelector('.echelle-curseur')"));
        if (!monte) await dormir(1000);
      }
      const morphEtat = await t.evalIn(`(() => {
        const b = [...document.querySelectorAll('button')].find(x => (x.textContent||'').includes('⟲ morph'));
        const etaitActif = !!b && b.className.includes('on');
        if (b && etaitActif) b.click();
        return { trouve: !!b, etaitActif }; })()`);
      // Attente du GRAPHE CHARGÉ : la ligne de statistiques annonce « N nœuds ».
      let noeudsCharges = 0;
      for (let k = 0; k < 30 && noeudsCharges < 2; k++) {
        await dormir(1000);
        noeudsCharges = +(String(await t.evalIn("(document.querySelector('.stats')?.innerText||'')"))
          .match(/(\d+)\s*nœuds/)?.[1] ?? 0);
      }
      // La sélection vient du CLAVIER (←/→ parcourir) : c'est un geste réel de
      // la grammaire commune, et il ne dépend d'aucune coordonnée d'écran. On le
      // répète tant que la barre n'est pas là, parce que le premier envoi peut
      // tomber avant que le graphe ne soit prêt.
      let barre = null;
      let essaisTouche = 0;
      for (; essaisTouche < 6 && !barre; essaisTouche++) {
        await t.touche("ArrowRight");
        for (let k = 0; k < 6 && !barre; k++) {
          await dormir(800);
          barre = await t.evalIn(`(() => {
            const fb = document.querySelector('.facetbar');
            if (!fb) return null;
            const totaux = fb.querySelector('.facetbar-totaux')?.innerText || '';
            const tete = fb.querySelector('.facetbar-qn')?.textContent || '';
            const nom = fb.querySelector('.facetbar-head b')?.textContent || '';
            const facettes = [...fb.querySelectorAll('.facetbar-facets .facet-grp')].map((g) => {
              const b = g.querySelector('button');
              return { label: (b?.textContent || '').trim(), dis: !!b?.disabled, titre: b?.title || '' };
            });
            return { totaux, tete, nom, facettes, texte: fb.innerText, situe: fb.querySelector('.facetbar-situe')?.innerText || '' };
          })()`);
        }
      }
      ecran = { morph: morphEtat, noeudsCharges, essaisTouche, barre };

      if (!barre) {
        P(false, `barre de facettes ABSENTE après ${essaisTouche} envois de ←/→ — état mesuré : scène montée ${monte}, ` +
          `morph ${morphEtat?.etaitActif ? "coupé par le contrôle" : "déjà inactif"}, ${noeudsCharges} nœud(s) chargé(s), ` +
          `bouton morph trouvé ${morphEtat?.trouve}`);
      } else {
        // ── 2. frères ET sœurs annoncés ─────────────────────────────────────
        const m = barre.totaux.match(/(\d+)\s*frères\s*·\s*(\d+)\s*co-fichiers\s*·\s*(\d+)\s*sœurs\s*·\s*(\d+)\s*dans le même cycle\s*·\s*(\d+)\s*homonymes/);
        const totaux = m ? { freres: +m[1], coFichiers: +m[2], soeurs: +m[3], cycle: +m[4], homonymes: +m[5] } : null;
        const indexés = (barre.totaux.match(/sur\s*(\d+)\s*nœuds chargés/) || [])[1];
        const msIndex = (barre.totaux.match(/index en ([\d.]+) ms/) || [])[1];
        ecran.totaux = totaux; ecran.indexes = indexés; ecran.msIndex = msIndex;
        P(!!totaux,
          `effectifs ANNONCÉS sans déplier : ${totaux ? `frères ${totaux.freres} · co-fichiers ${totaux.coFichiers} · ` +
            `sœurs ${totaux.soeurs} · cycle ${totaux.cycle} · homonymes ${totaux.homonymes}` : barre.totaux.slice(0, 140)}`);
        P(!!totaux && (totaux.freres > 0 || totaux.coFichiers > 0 || totaux.soeurs > 0),
          `fratrie NON VIDE (frères ${totaux?.freres ?? "?"} + sœurs ${totaux?.soeurs ?? "?"}) ` +
          `sur ${indexés ?? "?"} nœuds chargés${msIndex ? ` — index construit en ${msIndex} ms` : ""}`);
        P(/dossier\s*«/.test(barre.totaux) && /dossier/.test(barre.situe + barre.totaux),
          `le dossier porteur est nommé : ${(barre.situe || barre.totaux).replace(/\s+/g, " ").slice(0, 120)}`);

        // ── 3. affichés vs index : pas de troncature muette ────────────────
        const fr = barre.facettes.find((f) => f.label.startsWith("Frères"));
        const nAfficheTitre = +(fr?.label.match(/(\d+)\s*$/)?.[1] ?? NaN);
        ecran.facetteFreres = fr;
        const ouvert = await t.evalIn(`(() => {
          const fb = document.querySelector('.facetbar');
          const g = [...fb.querySelectorAll('.facetbar-facets .facet-grp')]
            .find((x) => (x.querySelector('button')?.textContent || '').startsWith('Frères'));
          if (!g) return null;
          g.querySelector('button').click();
          return true; })()`);
        // Le dépliage est un rendu React : on le LIT jusqu'à ce qu'il soit là
        // (avec une borne), et l'échec est DIAGNOSTIQUÉ au lieu d'être avalé —
        // « dépliage impossible » sans raison ne dit pas quoi corriger.
        let detail = null;
        for (let k = 0; k < 8 && !detail; k++) {
          await dormir(500);
          detail = await t.evalIn(`(() => {
            try {
              const fb = document.querySelector('.facetbar');
              if (!fb) return { err: 'barre absente' };
              const g = [...fb.querySelectorAll('.facetbar-facets .facet-grp')]
                .find((x) => (x.querySelector('button')?.textContent || '').startsWith('Frères'));
              if (!g) return { err: 'groupe Frères absent' };
              const items = g.querySelector('.facet-items');
              if (!items) return { err: 'dépli non rendu (bouton : ' + (g.querySelector('button')?.className || '?') + ')' };
              const puces = [...items.querySelectorAll('button.chip')];
              const lbl = items.querySelector('.qb-lbl');
              const totalLigne = +(((lbl && lbl.textContent) || '').match(/^(\\d+)/)?.[1] ?? NaN);
              return {
                ligne: (lbl && lbl.textContent) || '',
                puces: puces.length,
                lien: puces.filter((b) => (b.textContent || '').startsWith('→')).length,
                plus: puces.map((b) => (b.textContent || '').trim()).filter((x) => /^\\+\\d+ autres/.test(x)),
                totalLigne,
              };
            } catch (e) { return { err: String(e && e.message || e).slice(0, 120) }; }
          })()`);
        }
        ecran.detailFreres = detail;
        if (!detail || detail.err) P(false, `facette « Frères » : dépliage impossible — ${detail?.err || "aucune réponse"}`);
        else {
          const attendu = Math.min(detail.totalLigne, PLAFOND);
          P(detail.lien === attendu,
            `frères affichés ${detail.lien} = min(total ${detail.totalLigne}, plafond ${PLAFOND}) → ${attendu}`);
          P(detail.totalLigne === nAfficheTitre,
            `le total affiché (${detail.totalLigne}) est celui de la puce (${nAfficheTitre})`);
          const reste = detail.totalLigne - detail.lien;
          P(reste === 0 || detail.plus.length === 1 && detail.plus[0] === `+${reste} autres`,
            reste > 0 ? `troncature ANNONCÉE : « ${detail.plus[0] || "aucun dépliage"} » (reste ${reste})`
              : `aucune troncature (${detail.totalLigne} ≤ ${PLAFOND})`);
          // Le dépliage doit révéler EXACTEMENT le total annoncé.
          if (reste > 0) {
            await t.evalIn(`(() => {
              const fb = document.querySelector('.facetbar');
              const g = [...fb.querySelectorAll('.facetbar-facets .facet-grp')]
                .find((x) => (x.querySelector('button')?.textContent || '').startsWith('Frères'));
              [...g.querySelectorAll('button.chip')].find((b) => /^\\+\\d+ autres/.test((b.textContent||'').trim()))?.click();
              return true; })()`);
            await dormir(700);
            const apres = await t.evalIn(`(() => {
              const g = [...document.querySelector('.facetbar').querySelectorAll('.facetbar-facets .facet-grp')]
                .find((x) => (x.querySelector('button')?.textContent || '').startsWith('Frères'));
              return [...g.querySelectorAll('.facet-items button.chip')]
                .filter((b) => (b.textContent || '').startsWith('→')).length; })()`);
            ecran.deplie = apres;
            P(apres === detail.totalLigne,
              `dépliage : ${apres} frères révélés, total annoncé ${detail.totalLigne} → ` +
              `${apres === detail.totalLigne ? "exact" : "INCOHÉRENT"}`);
          }
        }

        // ── 4. cliquer un frère change la cible ────────────────────────────
        const avant = barre.tete;
        const clic = await t.evalIn(`(() => {
          const g = [...document.querySelector('.facetbar').querySelectorAll('.facetbar-facets .facet-grp')]
            .find((x) => (x.querySelector('button')?.textContent || '').startsWith('Frères'));
          const b = [...g.querySelectorAll('.facet-items button.chip')].find((x) => (x.textContent||'').startsWith('→'));
          if (!b) return null;
          const t = (b.textContent || '').trim();
          b.click();
          return t; })()`);
        await dormir(4000);
        const apresClic = await t.evalIn(`(() => {
          const fb = document.querySelector('.facetbar');
          if (!fb) return null;
          return {
            tete: fb.querySelector('.facetbar-qn')?.textContent || '',
            nom: fb.querySelector('.facetbar-head b')?.textContent || '',
            totaux: fb.querySelector('.facetbar-totaux')?.innerText || '',
          };
        })()`);
        ecran.clic = { avant, choisi: clic, apres: apresClic };
        P(!!clic && !!apresClic && apresClic.tete !== avant,
          `clic « ${clic || "aucun frère cliquable"} » : cible « ${avant.slice(-40)} » → ` +
          `« ${(apresClic?.tete || "(barre fermée)").slice(-40)} » ${apresClic && apresClic.tete !== avant ? "✓ changée" : "✗ INCHANGÉE"}`);
        P(!!apresClic,
          apresClic
            ? `la fratrie reste OUVERTE après l'ancrage (sélection vidée par ancrerAtom) : ` +
              `${apresClic.totaux.replace(/\s+/g, " ").slice(0, 130)}`
            : "la barre de facettes s'est FERMÉE au clic d'un frère (piège ancrerAtom non traité)");

        // ── 5. l'index est LOCAL : aucune requête pendant sa construction ──
        const reseau = await t.evalIn(`(() => {
          const F = window.__fratrie;
          if (!F) return { err: "window.__fratrie absent" };
          // Formes de qn RÉELLES, répliquées aux tailles du dépôt.
          const modeles = ${JSON.stringify((await qnsReels()).slice(0, 400))};
          if (!modeles.length) return { err: "aucun qn réel récupéré" };
          const taille = (n) => Array.from({ length: n }, (_, i) => ({
            id: String(i),
            qn: modeles[i % modeles.length],
            file_path: (i % 3 === 0) ? "/tmp/linux-6.6/drivers/net/ethernet/intel/e1000/e1000_hw.c" : undefined,
            community_id: i % 7, scc_id: i % 11,
          }));
          const avant = performance.getEntriesByType('resource').length;
          const mesures = [];
          for (const n of [160, 8000, 200000]) {
            const noeuds = taille(n);
            const t0 = performance.now();
            const idx = F.construireIndex(noeuds);
            mesures.push({ n, ms: +(performance.now() - t0).toFixed(1), internes: idx.ms,
              groupes: idx.fratrieParParent.size, noeuds: idx.n });
          }
          const apres = performance.getEntriesByType('resource').length;
          return { avant, apres, requetes: apres - avant, mesures };
        })()`);
        ecran.reseau = reseau;
        if (reseau?.err) P(false, `mesure de l'index : ${reseau.err}`);
        else {
          P(reseau.requetes === 0,
            `construction de l'index : ${reseau.requetes} requête(s) réseau ` +
            `(entrées performance ${reseau.avant} → ${reseau.apres}) — l'index est LOCAL`);
          for (const m of reseau.mesures) {
            lignes.push(`   ${String(m.n).padStart(6)} nœuds → ${String(m.ms).padStart(7)} ms ` +
              `(${m.groupes} groupes de parenté, ${m.noeuds} nœuds indexés)`);
          }
        }
      }
    } finally { await t.fermer(); }
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// 1b. LA PUCE DE FILTRE ALLUME VRAIMENT (vue Cosmos, étage graphe)
// ═══════════════════════════════════════════════════════════════════════════
let filtre = null;
{
  // ── ON PART DE L'ÉTAT PAR DÉFAUT, ET ON Y ARRIVE PAR UN CLIC RÉEL ───────
  // Question du propriétaire : « dans quelle face l'utilisateur voit-il les
  // frères ? » Réponse mesurée : la face Cosmos est la SEULE des trois qui ne
  // dépende pas de « ⟲ morph » (App ne monte Graph3D qu'avec le morph coupé, et
  // StoreOverview de même). Ce bloc part donc de l'URL SANS paramètre, clique
  // « 🌌 Cosmos » comme un utilisateur, puis désigne un élément par de VRAIS
  // événements souris. `portee` unique : le pont d'état d'interface restaure
  // sinon un document distant qui peut reposer une vue ou une échelle.
  const t = await onglet(`${BASE}/?portee=fratrie-check-${process.pid}`);
  if (!t) { P(false, "CDP : onglet refusé pour le contrôle de la fratrie dans le Cosmos"); }
  else {
    try {
      // 1. L'APPLICATION MONTÉE (sondée, pas supposée : elle met 7 à 10 s ici).
      let monte = false;
      for (let k = 0; k < 30 && !monte; k++) {
        monte = !!(await t.evalIn("!!document.querySelector('.echelle-curseur')"));
        if (!monte) await dormir(1000);
      }
      // 2. CLIC RÉEL sur « 🌌 Cosmos », depuis l'état par défaut.
      const clicVue = await t.evalIn(`(() => {
        const b = [...document.querySelectorAll('.viewswitch button')]
          .find(x => (x.textContent || '').includes('🌌 Cosmos'));
        if (!b) return null;
        b.click();
        return (b.textContent || '').trim();
      })()`);
      // 3. LA SCÈNE MONTÉE : `window.__pickSweep` est posé par la scène Cosmos —
      // sa présence prouve qu'elle est là, et son balayage dit ce qu'un clic
      // AURAIT choisi, cellule par cellule.
      const t0 = Date.now();
      let scenePrete = false;
      for (let k = 0; k < 30 && !scenePrete; k++) {
        scenePrete = !!(await t.evalIn("!!(window.__pickSweep && window.__fratrie)"));
        if (!scenePrete) await dormir(1000);
      }
      const attenteScene = Date.now() - t0;
      P(!!clicVue && scenePrete,
        `clic RÉEL sur « ${clicVue || "(bouton absent)"} » depuis l'état par défaut → scène Cosmos montée ` +
        `en ${attenteScene} ms` + (scenePrete ? "" : " — borne de 30 s atteinte"));
      if (!scenePrete) throw new Error("scène Cosmos non montée");

      const boite = await t.evalIn(`(() => {
        const c = document.querySelector('canvas');
        if (!c) return null;
        const r = c.getBoundingClientRect();
        return { x: r.left, y: r.top };
      })()`);
      const cliquer = async (z) => {
        const x = Math.round(boite.x + z.x), y = Math.round(boite.y + z.y);
        await t.send("Input.dispatchMouseEvent", { type: "mousePressed", x, y, button: "left", clickCount: 1 });
        await t.send("Input.dispatchMouseEvent", { type: "mouseReleased", x, y, button: "left", clickCount: 1 });
      };
      const zone = (b, g) => (b?.zones || []).find((z) => z.genre === g);
      const touchePlus = async (n) => { for (let i = 0; i < n; i++) await t.touche("+"); };
      const balayage = await t.evalIn("window.__pickSweep(48)");
      filtre = { attenteScene, balayage1: { cellules: balayage?.cellules, vides: balayage?.part_vide, genres: balayage?.par_genre } };

      // 4. DESCENDRE AVEC LA GRAMMAIRE COMMUNE (←/→ puis +/−), comme le fait
      // `emphasis-check.mjs` : c'est un geste réel de l'utilisateur, et il ne
      // dépend d'aucune coordonnée d'écran — donc il ne peut pas être un état
      // posé à la main.
      await t.touche("ArrowRight");
      await dormir(2000);
      await touchePlus(9);
      let texte = await t.texte();
      const attendreBloc = async (borne) => {
        for (let k = 0; k < borne; k++) {
          const pret = await t.evalIn(`(() => {
            const d = [...document.querySelectorAll('button.chip')].find(b => (b.textContent||'').trim() === 'parent');
            if (d && d.className.indexOf('on') < 0) d.click();
            return !!([...document.querySelectorAll('.qb-lbl')].find(x => (x.textContent||'').trim() === 'ressortir'));
          })()`);
          if (pret) return true;
          await dormir(2500);
          texte = await t.texte();
        }
        return false;
      };
      await attendreBloc(20);

      // 5. CLIC RÉEL SUR UNE BOULE, à l'étage où elles sont désignables.
      // C'est LE geste que la fratrie doit servir : désigner un élément et voir
      // ce qui l'entoure. On le fait avec de vrais événements souris, guidés par
      // le balayage de la scène elle-même.
      // On ATTEND qu'une boule soit désignable : selon l'état de la descente,
      // les premiers balayages ne rendent que des mondes (mesuré : genres
      // {"monde":170}), puis les boules apparaissent. On approche avec « + »
      // entre deux balayages — c'est exactement ce que fait un utilisateur.
      const dims = await t.evalIn(`(() => { const c = document.querySelector('canvas');
        const r = c.getBoundingClientRect(); return { w: r.width, h: r.height }; })()`);
      let bal2 = null, ze2 = null, clicBoule = null, apresClicBoule = null;
      for (let k = 0; k < 12 && !ze2; k++) {
        bal2 = await t.evalIn("window.__pickSweep(48)");
        const ensembles = (bal2?.zones || []).filter((z) => z.genre === "ensemble");
        // La zone la plus proche du CENTRE du canevas : un coin peut être un cas
        // limite (objet à peine visible), on veut le geste le plus ordinaire.
        ze2 = ensembles.sort((a, b) =>
          (Math.hypot(a.x - dims.w / 2, a.y - dims.h / 2) - Math.hypot(b.x - dims.w / 2, b.y - dims.h / 2)))[0] || null;
        if (!ze2) { await touchePlus(1); await dormir(2000); }
      }
      filtre = { ...filtre, balayage2: { cellules: bal2?.cellules, vides: bal2?.part_vide, genres: bal2?.par_genre } };
      if (ze2 && boite) {
        await cliquer(ze2);
        clicBoule = { ...ze2 };
        await touchePlus(4);
        for (let k = 0; k < 20 && !apresClicBoule; k++) {
          await dormir(2000);
          apresClicBoule = await t.evalIn(`(() => {
            const l = [...document.querySelectorAll('.qb-lbl')].find(x => /fratrie de /.test(x.textContent || ''));
            if (l) return { ligne: (l.textContent || '').replace(/\\s+/g, ' ').trim(),
              source: /nœud visé/.test(l.textContent || '') ? 'nœud visé'
                : /centre du graphe affiché/.test(l.textContent || '') ? 'centre du graphe'
                : /le plus entouré/.test(l.textContent || '') ? 'le plus entouré (centre non-nœud)' : '?' };
            const attente = [...document.querySelectorAll('.q-intro, .qb-lbl')]
              .find(x => /^Fratrie/.test((x.textContent || '').trim()));
            return attente ? { ligne: (attente.textContent || '').replace(/\\s+/g, ' ').trim(), source: 'raison' } : null;
          })()`);
        }
      }
      filtre = { ...filtre, clicBoule, apresClicBoule };
      P(!!clicBoule, clicBoule
        ? `clic RÉEL sur une boule en (${clicBoule.x}, ${clicBoule.y}) — genres désignables ` +
          `${JSON.stringify(bal2?.par_genre)}`
        : `aucune boule désignable (genres ${JSON.stringify(bal2?.par_genre)})`);
      if (apresClicBoule) {
        P(true, `panneau LU après le clic sur une boule (${apresClicBoule.source}) : ` +
          `« ${apresClicBoule.ligne.slice(0, 165)} »`);
      } else {
        P(false, "aucune fratrie affichée 40 s après le clic sur une boule");
      }

      // 6. CLIC RÉEL SUR UN NŒUD, quand la nuée en expose un : balayage FIN,
      // parce que les nœuds sont des POINTS (un pas de 48 px les manque).
      let apresClicNoeud = null, balayageFin = null;
      const bal3 = await t.evalIn("window.__pickSweep(12)");
      balayageFin = { cellules: bal3?.cellules, vides: bal3?.part_vide, genres: bal3?.par_genre };
      const zn = zone(bal3, "noeud");
      if (zn) {
        await cliquer(zn);
        for (let k = 0; k < 14 && !apresClicNoeud; k++) {
          await dormir(1500);
          apresClicNoeud = await t.evalIn(`(() => {
            const l = [...document.querySelectorAll('.qb-lbl')]
              .find(x => /fratrie de /.test(x.textContent || '') && /nœud visé/.test(x.textContent || ''));
            return l ? (l.textContent || '').replace(/\\s+/g, ' ').trim() : null;
          })()`);
        }
      }
      filtre = { ...filtre, balayageFin, apresClicNoeud };
      if (apresClicNoeud) {
        P(true, `clic RÉEL sur un nœud → « ${apresClicNoeud.slice(0, 150)} »`);
      } else {
        // Aucun point de nuée n'est désignable à ce cadrage (les boules et les
        // mondes occupent la visée : le balayage fin ne rend que ces genres).
        // Ce n'est pas un échec du produit, mais ce n'est pas une preuve non
        // plus : on l'écrit noir sur blanc plutôt que de le passer sous silence.
        lignes.push(`   NON VÉRIFIÉ : variante « nœud visé » — aucun point de nuée désignable au balayage fin ` +
          `(${bal3?.cellules} cellules, genres ${JSON.stringify(bal3?.par_genre)}). La fratrie affichée ` +
          `ci-dessus vient donc du CENTRE DU GRAPHE (geste : clic sur une boule), pas d'un nœud visé.`);
      }

      const puces = await t.evalIn(`(() => {
        const lbl = [...document.querySelectorAll('.qb-lbl')].find(x => (x.textContent||'').trim() === 'ressortir');
        if (!lbl) return null;
        const groupes = [...document.querySelectorAll('.facet-grp, .chip')];
        // les puces suivent l'étiquette « ressortir » dans le même bloc
        const bloc = lbl.parentElement;
        return [...bloc.querySelectorAll('button.chip')]
          .filter(b => /\\d/.test(b.textContent||''))
          .map(b => ({ texte: (b.textContent||'').trim(), titre: b.title||'' }));
      })()`);
      filtre = { puces };
      if (!puces || !puces.length) {
        P(false, `vue Cosmos : aucune puce de filtre « ressortir » trouvée (graphe non chargé ?) — ${texte.slice(0, 100).replace(/\n/g, " ")}`);
      } else {
        const cible = puces[0];
        const annonce = +(cible.titre.match(/\((\d+) nœuds\)/) || [])[1] ?? NaN;
        await t.evalIn(`(() => {
          const lbl = [...document.querySelectorAll('.qb-lbl')].find(x => (x.textContent||'').trim() === 'ressortir');
          const bloc = lbl.parentElement;
          const b = [...bloc.querySelectorAll('button.chip')]
            .find(x => (x.textContent||'').trim() === ${JSON.stringify(cible.texte)});
          b?.click(); return true; })()`);
        // ── ÉCHANTILLONNAGE, PLUTÔT QU'UNE LECTURE UNIQUE ───────────────────
        // La valeur est publiée par la boucle de rendu, et la puce peut être
        // remise à zéro par un rechargement de graphe (l'effet qui recalcule
        // les groupes fait `setFiltreGraphe(null)`). Une lecture unique
        // mesurerait donc « pas de filtre » en croyant mesurer la puce. On
        // échantillonne, et l'on retient l'échantillon de L'ÉTIQUETTE CLIQUEÉE.
        const echantillons = [];
        const lu0 = () => t.evalIn("window.__etages?.filtre || null");
        for (let k = 0; k < 12; k++) {
          const s = await lu0();
          if (s) echantillons.push(s);
          await dormir(300);
        }
        const etiquette = cible.texte.replace(/\s*\d+\s*$/, "").trim();
        const mesure = echantillons.find((s) => s.etiquette === etiquette)
          || echantillons.slice().reverse().find((s) => s.etiquette) || null;
        const lu = (await t.texte()).match(/ressortent[^\n]*/)?.[0] || "(readout absent)";
        filtre = { ...filtre, cible, annonce, mesure, lu, echantillons: echantillons.length };
        P(!!mesure && mesure.etiquette === etiquette,
          `puce « ${cible.texte} » cliquée — mesure retenue : étiquette « ${mesure?.etiquette ?? "(aucune)"} » ` +
          `sur ${mesure?.total ?? "?"} nœuds (${echantillons.length} échantillons prélevés)`);
        P(!!mesure && mesure.etiquette === etiquette && mesure.allumes > 0,
          `la puce ALLUME ${mesure?.allumes ?? "?"} nœud(s) ` +
          `(annoncé : ${Number.isFinite(annonce) ? annonce : "?"}) — ` +
          `${mesure?.allumes > 0 ? "elle n'est plus morte" : "MORTE ✗"}`);
        P(!!mesure && Number.isFinite(annonce) && mesure.allumes === annonce,
          `effectif allumé ${mesure?.allumes} = effectif annoncé ${annonce} → ` +
          `${mesure?.allumes === annonce ? "la puce dit vrai" : "INCOHÉRENT"}`);
        lignes.push(`   readout : ${lu}`);
      }

      // ── 6. LES FRÈRES DE LA SCÈNE SONT NOMMÉS, ET CLIQUABLES ─────────────
      // « Les frères restent visibles en retrait » était DÉJÀ vrai (opacité
      // `ouvI × 0,26`) — mais aucun n'était nommé. On exige donc : une puce par
      // boule réellement dessinée (aucune muette), l'effectif réel sur la puce,
      // et un clic qui CADRE (la caméra part : mesuré sur la taille apparente
      // du monde, exportée par la boucle).
      const boules = await t.evalIn(`(() => {
        const lbl = [...document.querySelectorAll('.qb-lbl')]
          .find(x => /frères dans ce monde/.test(x.textContent || ''));
        if (!lbl) return null;
        const chips = [...lbl.parentElement.querySelectorAll('button.chip')];
        return {
          ligne: (lbl.textContent || '').trim(),
          puces: chips.length,
          exemple: chips.length ? (chips[chips.length - 1].textContent || '').trim() : '',
        };
      })()`);
      const poupees = await t.evalIn("window.__etages?.interieur?.poupees || null");
      filtre = { ...filtre, boules, poupees };
      if (!boules || !poupees) {
        P(false, `vue Cosmos : les frères de la scène ne sont pas nommés — ${boules ? JSON.stringify(boules).slice(0, 80) : "étiquette « frères dans ce monde » absente"}`);
      } else {
        P(poupees.freres_nommes === poupees.freres_boules && poupees.freres_boules > 0,
          `frères de la scène : ${poupees.freres_nommes} nommés pour ${poupees.freres_boules} boules ` +
          `dessinées (${Number(poupees.freres_effectif || 0).toLocaleString("fr-FR")} nœuds) — aucun frère muet`);
        P(boules.puces === poupees.freres_boules,
          `puces affichées ${boules.puces} = boules ${poupees.freres_boules} · « ${boules.ligne.slice(0, 70)} »`);
        const avant = await t.evalIn("window.__etages?.interieur?.apparent ?? null");
        const clic = await t.evalIn(`(() => {
          const lbl = [...document.querySelectorAll('.qb-lbl')]
            .find(x => /frères dans ce monde/.test(x.textContent || ''));
          if (!lbl) return null;
          const chips = [...lbl.parentElement.querySelectorAll('button.chip')];
          if (!chips.length) return null;
          const b = chips[chips.length - 1];
          const t = (b.textContent || '').trim();
          b.click();
          return t; })()`);
        await dormir(4000);
        const apres = await t.evalIn("window.__etages?.interieur?.apparent ?? null");
        P(!!clic && avant != null && apres != null && avant !== apres,
          `clic « ${clic} » : cadrage du monde ${Number(avant).toFixed(3)} → ${Number(apres).toFixed(3)} — ` +
          `${avant !== apres ? "la caméra a suivi" : "AUCUN EFFET"}`);
      }
    } finally { await t.fermer(); }
  }
}

// ═══════════════════════════════════════════════════════════════════════════
// 7. OÙ L'UTILISATEUR VOIT-IL LA FRATRIE ? — MESURÉ DANS L'ÉTAT PAR DÉFAUT
// ═══════════════════════════════════════════════════════════════════════════
// Question du propriétaire : « dans quelle face voit-on les frères et sœurs, et
// par quel geste ? » On ne répond pas par lecture de code : on ouvre l'URL SANS
// paramètre (l'état par défaut), on lit la face montée et l'état de « ⟲ morph »,
// et l'on nomme les gestes VÉRIFIÉS (ceux qui ont produit un PASS ci-dessus).
let defaut = null;
{
  const t = await onglet(BASE + "/");
  if (t) {
    try {
      let monte = false;
      for (let k = 0; k < 30 && !monte; k++) {
        monte = !!(await t.evalIn("!!document.querySelector('.echelle-curseur')"));
        if (!monte) await dormir(1000);
      }
      defaut = await t.evalIn(`(() => {
        const boutons = [...document.querySelectorAll('.viewswitch button')];
        const actif = (t) => {
          const b = boutons.find((x) => (x.textContent || '').includes(t));
          return b ? b.className.includes('on') : null;
        };
        return {
          face: actif('🌌 Cosmos') ? 'cosmos' : actif('🔺 Graphe') ? 'graphe' : 'monde',
          morph: actif('⟲ morph'),
          cosmosScene: !!window.__pickSweep,
          sceneMonde: !!document.querySelector('.monde-focus, .ov-mode'),
          sceneGraphe: !!document.querySelector('.canvas-host'),
          stageContinu: !!document.querySelector('.overview') && !document.querySelector('.canvas-host'),
          barreFacettes: !!document.querySelector('.facetbar'),
          instrument: typeof window.__fratrie?.parentDe === 'function',
        };
      })()`);
      P(!!defaut?.instrument,
        `état par défaut (aucun paramètre) : face « ${defaut?.face} », « ⟲ morph » ${defaut?.morph ? "ACTIF" : "inactif"} — ` +
        `scène montée : ${defaut?.cosmosScene ? "Cosmos" : defaut?.sceneGraphe ? "Graph3D" : defaut?.sceneMonde ? "StoreOverview" : defaut?.stageContinu ? "StageContinu (le morph)" : "?"}` +
        ` · barre de facettes ${defaut?.barreFacettes ? "présente" : "ABSENTE"}`);
      lignes.push("   pourquoi : App.tsx ne monte FacetBar que si « !monde && !cosmos && selectedNode »,");
      lignes.push("   et Graph3D n'est monté que si « ⟲ morph » est INACTIF — dans l'état par défaut, aucun des deux n'est vrai.");
    } finally { await t.fermer(); }
  }
}

// ── RAPPORT ────────────────────────────────────────────────────────────────
console.log("FRATRIE — les frères et les sœurs d'un élément sélectionné\n");
console.log(`service : ${BASE}  ·  cible : ${CIBLE.store} / ${CIBLE.centre}  ·  plafond d'affichage : ${PLAFOND}`);
console.log(`identité : ${AUTH}\n`);
console.log(`— unicité de la règle de parenté —`);
if (unicite && !unicite.err) {
  console.log(`  instrument attendu ${unicite.attenteMs} ms (sondes, pas une durée fixe)`);
  console.log(`  qn testés : ${unicite.n} (dont ${unicite.avecFile} avec marqueur __file__)`);
  console.log(`  paires (qn, file_path) réelles : ${unicite.paires}`);
  console.log(`  divergences ancienne/étiquette : ${unicite.divAnciennes}`);
  console.log(`  divergences étiquette/parentDe  : ${unicite.divNouvelle}`);
}
console.log(`\n— à l'écran —`);
for (const l of lignes) console.log("  " + l);
console.log(
  `\n${bad ? `ÉCHEC : ${bad} contrôle(s) non conforme(s)` :
    "OK : règle de parenté UNIQUE, fratrie affichée avec son total, frères cliquables, index local"}`);
process.exit(bad ? 1 : 0);
