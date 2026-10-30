#!/usr/bin/env node
// ── GARDE PROTOCOLAIRE DU 8432 ──────────────────────────────────────────────
// Ce que ce script fige en non-régression, chaque point ayant été MESURÉ
// défaillant le 2026-09-13 sur le service vivant :
//   1. traversée de chemin sur /app/* (curl --path-as-is rendait 200 + le
//      contenu de /etc/hostname) ;
//   2. aucune compression malgré Accept-Encoding (bundle 1 286 462 o) ;
//   3. Connection: close systématique (2 connexions TCP pour 2 requêtes) ;
//   4. HEAD refusé en 405 avec un corps ;
//   5. aucun en-tête Date ;
//   6. aucun ETag / 304 sur les statiques hachés ;
//   7. SSE : ni retry:, ni battement de cœur, X-Accel-Buffering absent ;
//   8. ligne de requête absente → AUCUNE réponse (client pendu).
//
// Usage : node scripts/protocole-check.mjs [base] [--sse]
//   base par défaut : http://192.168.1.103:8433 (instance isolée)
// Sortie : lignes PASS/FAIL, code de sortie 1 si au moins un FAIL.
import { argv, exit } from "node:process";
import net from "node:net";

const BASE = argv[2] || "http://192.168.1.103:8433";
const U = new URL(BASE);
const HOST = U.hostname;
const PORT = Number(U.port || 80);

let ko = 0;
let ignores = 0;
const ligne = (ok, nom, detail) => {
  if (!ok) ko++;
  console.log(`${ok ? "PASS" : "FAIL"}  ${nom.padEnd(52)} ${detail}`);
};
/**
 * Un contrôle NON APPLICABLE n'est pas un échec.
 *
 * ── POURQUOI (2026-09-13) ──────────────────────────────────────────────────
 * Le service 8432 a été relancé avec l'IAM actif (`OBRAIN_IAM=1`) : `/` répond
 * 303 vers `/login` (« connexion requise ») et `/api/**` répond 401. Les
 * contrôles de PAGE (bundle, ETag, `no-cache`) n'ont alors plus d'objet : il n'y
 * a pas de bundle à compresser sur une redirection. Les compter comme des échecs
 * ferait crier une porte sur un contexte qu'elle ne mesure pas — exactement le
 * travers que ce fichier corrige par ailleurs. Ils sont donc SKIP, avec la
 * raison ET le geste qui les rendrait applicables (une session).
 */
const nonApplicable = (nom, raison) => {
  ignores++;
  console.log(`${"SKIP".padEnd(6)}${nom.padEnd(52)} ${raison}`);
};

async function req(chemin, init = {}) {
  const r = await fetch(BASE + chemin, init);
  const corps = await r.arrayBuffer();
  return { r, corps, tete: Object.fromEntries(r.headers) };
}

/** Le service exige-t-il une session ? (redirection de la page d'entrée) */
const devantIam = await (async () => {
  try {
    const r = await fetch(BASE + "/", { redirect: "manual" });
    if (r.status >= 300 && r.status < 400 && /\/login/.test(r.headers.get("location") || "")) {
      return { oui: true, code: r.status, vers: r.headers.get("location") };
    }
    return { oui: false };
  } catch (e) {
    return { oui: false, erreur: String(e).slice(0, 60) };
  }
})();
console.log(`# ${BASE} — service ${devantIam.oui ? `derrière IAM (${devantIam.code} → ${devantIam.vers})` : "ouvert"}\n`);

// ── 1. Traversée de chemin ─────────────────────────────────────────────────
// On envoie la requête BRUTE : `fetch` normalise `..`, curl ne le fait qu'avec
// --path-as-is — il faut donc parler le protocole soi-même.
function brut(requete, { attendre = 400 } = {}) {
  return new Promise((resolve) => {
    const s = net.connect(PORT, HOST);
    let buf = "";
    const fin = (raison) => { try { s.destroy(); } catch {} resolve({ buf, raison }); };
    s.setTimeout(attendre, () => fin("timeout"));
    s.on("connect", () => s.write(requete));
    s.on("data", (d) => { buf += d.toString("latin1"); });
    s.on("end", () => fin("end"));
    s.on("error", (e) => fin("err:" + e.code));
  });
}

/**
 * Compte les RÉPONSES d'un flux HTTP/1.1 en marchant de `Content-Length` en
 * `Content-Length`.
 *
 * ── POURQUOI PAS UNE REGEX (défaut de cet instrument, corrigé le 2026-09-13) ─
 * Deux réponses servies sur la même connexion se SUIVENT SANS SÉPARATEUR : la
 * ligne de statut de la seconde commence juste après le corps de la première,
 * donc `^HTTP\/1\.[01]` en mode multiligne ne la voit pas. La sonde annonçait
 * « 1 réponse » là où le serveur en avait écrit deux — j'ai failli déclarer
 * cassé un keep-alive qui marchait. Ce qui a tranché : la même requête envoyée
 * par une sonde Python à regex NON ancrée comptait 2.
 */
function compterReponses(buf) {
  let i = 0, n = 0;
  while (i < buf.length) {
    const fin = buf.indexOf("\r\n\r\n", i);
    if (fin < 0) break;
    const bloc = buf.slice(i, fin);
    if (!/^HTTP\/1\.[01] \d{3}/.test(bloc)) break;
    const cl = /content-length:\s*(\d+)/i.exec(bloc);
    n++;
    // Sans `Content-Length` (flux SSE), on ne sait pas où finit la réponse : on
    // s'arrête au lieu de deviner.
    if (!cl) break;
    i = fin + 4 + Number(cl[1]);
  }
  return n;
}

{
  // ── LA PROFONDEUR FIXE ÉTAIT UN FAUX POSITIF (corrigé le 2026-09-15) ──────
  // Le contrôle envoyait SEPT `../`, valeur calibrée sur l'ancienne racine
  // servie (`…/banc-obrain-j5/packages/obrain-dashboard/dist`). Quand le `dist`
  // est devenu un LIEN vers le magasin de versions
  // (`…/.local/share/obrain-harness/ui/current`, un chemin PLUS PROFOND), sept
  // niveaux ont cessé d'atteindre `/etc` : le contrôle est passé à PASS… alors
  // que la fuite était intacte, simplement décalée. Vérifié : 9, 11, 13 et 15
  // niveaux rendaient tous `200` + le contenu de `/etc/hostname`.
  //
  // On ne teste donc plus UNE profondeur : on teste la CAPACITÉ d'échappement,
  // de 7 à 20 niveaux. Une seule réussite suffit à faire échouer le contrôle.
  const profondeurs = [7, 9, 11, 13, 15, 18, 20];
  const fuites = [];
  let refus = 0;
  for (const n of profondeurs) {
    const chemin = "/app/" + "../".repeat(n) + "etc/hostname";
    const { buf } = await brut(`GET ${chemin} HTTP/1.1\r\nHost: ${HOST}\r\nConnection: close\r\n\r\n`);
    const code = (buf.match(/^HTTP\/1\.[01] (\d{3})/) || [])[1];
    const corps = buf.split("\r\n\r\n")[1] || "";
    const contenu = /n3|localhost|root:/i.test(corps);
    if (code === "404") { refus++; continue; }
    if (code === "200") fuites.push(`${n} niveaux → 200${contenu ? " + CONTENU" : ""}`);
  }
  ligne(fuites.length === 0, "traversée /app/… refusée À TOUTE PROFONDEUR (7→20)",
    fuites.length
      ? `FUITE : ${fuites.join(" · ")} (${refus}/${profondeurs.length} profondeurs refusées)`
      : `${refus}/${profondeurs.length} profondeurs refusées (404)`);
}

// ── 2. Compression ─────────────────────────────────────────────────────────
// ⚠ MESURER LA TAILLE PAR `fetch` EST TROMPEUR (défaut de cet instrument,
// corrigé le 2026-09-13) : `fetch` dégonfle lui-même quand `Content-Encoding`
// est présent, donc `arrayBuffer().byteLength` rend la taille D'APRÈS
// décompression — identique à `identity`, quel que soit le gain réel. On lit
// donc la taille ANNONCÉE (`Content-Length`, posée par le serveur sur le corps
// compressé), seul chiffre qui décrit le fil.
{
  const page = await req("/");
  const html = new TextDecoder().decode(page.corps);
  const bundle = (html.match(/\/app\/assets\/[^"']+\.js/) || [])[0];
  if (!bundle) {
    if (devantIam.oui) {
      nonApplicable("compression du bundle",
        "IAM actif : `/` redirige, il n'y a pas de bundle à compresser sans session (voir §6)");
    } else {
      ligne(false, "compression du bundle", "aucun bundle trouvé dans /");
    }
  } else {
    const nu = await req(bundle, { headers: { "Accept-Encoding": "identity" } });
    const gz = await req(bundle, { headers: { "Accept-Encoding": "gzip" } });
    const enc = gz.tete["content-encoding"] || "—";
    const fil = Number(gz.tete["content-length"] ?? gz.corps.byteLength);
    const sans = Number(nu.tete["content-length"] ?? nu.corps.byteLength);
    const gain = sans ? (1 - fil / sans) * 100 : 0;
    ligne(enc.includes("gzip") && fil < sans * 0.8,
      "bundle servi en gzip",
      `${sans} → ${fil} o sur le fil (${gain.toFixed(1)} %), Content-Encoding: ${enc}`);
    ligne(/\baccept-encoding\b/i.test(gz.tete["vary"] || ""),
      "Vary: Accept-Encoding présent", `Vary: ${gz.tete["vary"] || "—"}`);
    // Un JSON d'API doit aussi être compressé (il est très répétitif).
    //
    // ⚠ MAIS PAS À N'IMPORTE QUEL PRIX (corrigé le 2026-09-15) : la lignée
    // consolidée applique un **seuil de compression de 512 o** (compresser
    // 200 o coûte plus qu'il ne rapporte), là où la lignée servie compresse
    // tout. Le contrôle exigeait « gzip » sans regarder la taille : il
    // accusait donc une POLITIQUE, pas une régression. Sous le seuil, la mesure
    // n'est pas concluante — on le DIT au lieu de le compter comme un échec.
    const j = await req("/api/viz/modules", { headers: { "Accept-Encoding": "gzip" } });
    const jf = Number(j.tete["content-length"] ?? j.corps.byteLength);
    if (devantIam.oui && j.r.status === 401) {
      nonApplicable("JSON d'API compressé", "IAM actif : `/api/**` répond 401 sans session");
    } else if (jf < 512) {
      nonApplicable("JSON d'API compressé",
        `corps de ${jf} o < seuil de compression (512 o sur la lignée consolidée) — non concluant`);
    } else {
      ligne((j.tete["content-encoding"] || "").includes("gzip"),
        "JSON d'API compressé",
        `${jf} o sur le fil, Content-Encoding: ${j.tete["content-encoding"] || "—"}`);
    }
  }
}

// ── 3. Keep-alive : DEUX requêtes sur UNE connexion ────────────────────────
{
  const deux = await brut(
    `GET /api/stores HTTP/1.1\r\nHost: ${HOST}\r\nConnection: keep-alive\r\n\r\n` +
    `GET /api/stores HTTP/1.1\r\nHost: ${HOST}\r\nConnection: close\r\n\r\n`
  );
  const nb = compterReponses(deux.buf);
  const entete = (deux.buf.match(/^HTTP\/1\.[01] \d{3}.*?\r\n(?:[\s\S]*?)\r\n\r\n/) || [])[0] || "";
  const promet = /Connection: keep-alive/i.test(entete);
  ligne(nb === 2, "2 requêtes pipelinées, 2 réponses",
    `réponses=${nb} (${deux.raison}), 1re annonce : ${promet ? "keep-alive" : "close/absent"}`);
}

// ── 4. HEAD ────────────────────────────────────────────────────────────────
{
  const r = await fetch(BASE + "/", { method: "HEAD" });
  const corps = (await r.arrayBuffer()).byteLength;
  ligne(r.status === 200 && corps === 0, "HEAD / → 200 sans corps",
    `code=${r.status}, corps=${corps} o, Content-Length=${r.headers.get("content-length")}`);
}
{
  const r = await fetch(BASE + "/zzz-inexistant", { method: "HEAD" });
  ligne(r.status === 404, "HEAD sur route absente → 404", `code=${r.status}`);
}

// ── 5. Date + Allow + rigueur ──────────────────────────────────────────────
{
  const { tete } = await req("/");
  ligne(!!tete["date"], "en-tête Date présent", `Date: ${tete["date"] || "ABSENT"}`);
}
{
  const r = await fetch(BASE + "/api/stores", { method: "PUT" });
  ligne(r.status === 405 && !!r.headers.get("allow"),
    "405 porte Allow:", `code=${r.status}, Allow: ${r.headers.get("allow") || "ABSENT"}`);
}
{
  // Ligne de requête vide : le serveur doit répondre 400, jamais se taire.
  const vide = await brut("\r\n\r\n", { attendre: 3000 });
  const code = (vide.buf.match(/^HTTP\/1\.[01] (\d{3})/) || [])[1];
  ligne(code === "400", "ligne de requête vide → 400",
    `code=${code ?? "AUCUNE RÉPONSE (" + vide.raison + ")"}`);
}
{
  const sansHost = await brut("GET / HTTP/1.1\r\nConnection: close\r\n\r\n", { attendre: 3000 });
  const code = (sansHost.buf.match(/^HTTP\/1\.[01] (\d{3})/) || [])[1];
  ligne(code === "400", "HTTP/1.1 sans Host → 400", `code=${code ?? "aucune réponse"}`);
}

// ── 6. Revalidation ETag / 304 ─────────────────────────────────────────────
if (devantIam.oui) {
  nonApplicable("ETag / 304 sur page et bundle",
    "IAM actif : `/` est une redirection, `/app/assets` n'est pas servi sans session");
  nonApplicable("page d'entrée revalidable (pas no-store)",
    "IAM actif : le `no-store` de la redirection EST correct (une redirection ne se cache pas)");
  // À défaut, on mesure ce qui EXISTE sans session : la page de connexion, servie
  // par le même transport. Si elle est trop petite pour être compressible, on le
  // dit au lieu d'inventer un verdict.
  //
  // ⚠ LA RÉFÉRENCE DOIT DEMANDER `identity` EXPLICITEMENT : `fetch` envoie
  // `Accept-Encoding: gzip` PAR DÉFAUT. Sans ce réglage, la « taille sans
  // compression » était déjà compressée et le contrôle annonçait « 720 → 720,
  // aucun gain » sur une compression qui marchait (1307 → 720, magie `1f8b`
  // vérifiée à la main). Troisième fois dans ce chantier qu'une mesure de
  // compression est fausse faute d'avoir contrôlé l'en-tête de négociation.
  const login = await req("/login", { headers: { "Accept-Encoding": "identity" } });
  const taille = Number(login.tete["content-length"] ?? login.corps.byteLength);
  const gz = await req("/login", { headers: { "Accept-Encoding": "gzip" } });
  const fil = Number(gz.tete["content-length"] ?? gz.corps.byteLength);
  if (taille > 512) {
    ligne((gz.tete["content-encoding"] || "").includes("gzip") && fil < taille,
      "compression de la page de connexion (sans session)",
      `${taille} → ${fil} o sur le fil, Content-Encoding: ${gz.tete["content-encoding"] || "—"}`);
  } else {
    nonApplicable("compression de la page de connexion", `page de ${taille} o — sous le seuil de compression`);
  }
} else {
  const html = await req("/");
  const bundle = new TextDecoder().decode(html.corps).match(/\/app\/assets\/[^"']+\.js/)?.[0];
  for (const [nom, chemin] of [["page /", "/"], ["bundle haché", bundle]]) {
    if (!chemin) continue;
    const a = await req(chemin);
    const etag = a.tete["etag"];
    if (!etag) { ligne(false, `ETag sur ${nom}`, "aucun ETag"); continue; }
    const b = await req(chemin, { headers: { "If-None-Match": etag } });
    ligne(b.r.status === 304, `If-None-Match sur ${nom} → 304`,
      `code=${b.r.status}, corps=${b.corps.byteLength} o, ETag: ${etag}`);
  }
  // La page d'entrée doit être REVALIDABLE (no-cache), pas no-store.
  const cc = html.tete["cache-control"] || "";
  ligne(/no-cache|max-age/.test(cc) && !/no-store/.test(cc),
    "page d'entrée revalidable (pas no-store)", `Cache-Control: ${cc || "—"}`);
}

// ── 7. SSE : retry, battement, pas de compression ──────────────────────────
// Opt-in (`--sse`) : cette sonde OUVRE UNE GÉNÉRATION sur le LLM local. On ne
// la déclenche donc pas par accident sur le service de l'utilisateur.
if (argv.includes("--sse")) {
  const ctl = new AbortController();
  const t = setTimeout(() => ctl.abort(), 12_000);
  try {
    const r = await fetch(
      BASE + "/api/engine/chat?deja=&q=ping&store=" + encodeURIComponent(argv[3] || "Runtime dev (engrams)"),
      { signal: ctl.signal, headers: { Accept: "text/event-stream" } }
    );
    const tete = r.headers;
    const lecteur = r.body.getReader();
    let texte = "";
    const deadline = Date.now() + 6000;
    while (Date.now() < deadline) {
      const { value, done } = await lecteur.read();
      if (done) break;
      texte += new TextDecoder().decode(value);
      if (/retry:/.test(texte) && texte.length > 40) break;
    }
    try { await lecteur.cancel(); } catch {}
    ligne(/text\/event-stream/.test(tete.get("content-type") || ""),
      "SSE : type de contenu", `Content-Type: ${tete.get("content-type") || "—"}`);
    ligne(!tete.get("content-encoding"), "SSE non compressé",
      `Content-Encoding: ${tete.get("content-encoding") || "aucun"}`);
    ligne(/retry:\s*\d+/.test(texte), "SSE : trame retry présente",
      (/retry:\s*\d+/.exec(texte) || ["ABSENTE"])[0]);
    ligne(/x-accel-buffering/i.test(JSON.stringify(tete)),
      "SSE : X-Accel-Buffering", `X-Accel-Buffering: ${tete.get("x-accel-buffering") || "ABSENT"}`);
    console.log(`      (premières trames SSE : ${JSON.stringify(texte.slice(0, 120))})`);
  } catch (e) {
    ligne(false, "SSE joignable", `échec : ${String(e).slice(0, 80)}`);
  } finally { clearTimeout(t); }
}

// ── 8. Non-régression : rien ne doit plus jamais pendre ────────────────────
{
  const t0 = performance.now();
  const r = await fetch(BASE + "/api/stores");
  await r.arrayBuffer();
  ligne(performance.now() - t0 < 3000, "/api/stores répond",
    `${(performance.now() - t0).toFixed(0)} ms, code=${r.status}`);
}

console.log(`\n${ko === 0 ? "PROTOCOLE : TOUT PASSE" : `PROTOCOLE : ${ko} ÉCHEC(S)`}` +
  (ignores ? `  (${ignores} contrôle(s) NON APPLICABLE(S) — voir SKIP, ils ne comptent pas comme des échecs)` : ""));
exit(ko === 0 ? 0 : 1);
