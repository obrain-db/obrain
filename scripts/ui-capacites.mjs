#!/usr/bin/env node
// ── CAPACITÉS D'UN BUILD DE DASHBOARD ───────────────────────────────────────
// Mesure ce qu'un dist SAIT FAIRE, en ne regardant QUE ce que la page sert
// réellement : l'index.html, les assets qu'il désigne, et la fermeture des
// morceaux chargés depuis ceux-ci (les chunks dynamiques sont hashés et cités
// par le bundle qui les importe).
//
// POURQUOI CE SCRIPT EXISTE (2026-09-15)
// Le 14/09 à 20:03, un `pnpm build` dans le worktree servi a réécrit
// dist/index.html vers un bundle bâti depuis une lignée qui n'avait NI le rendu
// markdown NI KaTeX — et le redémarrage de 21:50 a rendu la substitution
// effective. Personne ne l'a vu : un bundle est un fichier de 1,3 Mo, sa
// capacité ne se lit pas à l'œil. On la MESURE donc, et on refuse de promouvoir
// une UI qui a perdu une capacité que la précédente avait.
//
// ⚠ DÉFAUT D'INSTRUMENT CORRIGÉ ICI MÊME (2026-09-15) : la première version
// balayait TOUS les .js/.css du dossier d'assets. Or ce dossier est un
// cimetière : il accumule les bundles de tous les builds passés, dont ceux
// d'une autre lignée. Résultat : la mesure annonçait « markdown présent » sur
// un dist qui ne le servait PAS — le cimetière parlait à la place du vivant.
// Mesurer la fermeture ATTEIGNABLE est la seule mesure qui décrive la page.
//
// Sortie : JSON sur stdout. Code non nul si l'index.html est illisible.
import { readFileSync, existsSync, statSync } from "node:fs";
import { join } from "node:path";
import { createHash } from "node:crypto";

const racine = process.argv[2];
if (!racine) {
  console.error("usage: ui-capacites.mjs <dist-dir>");
  process.exit(2);
}
if (!existsSync(join(racine, "index.html"))) {
  console.error(`ui-capacites: pas d'index.html dans ${racine}`);
  process.exit(2);
}

const sha = (buf) => createHash("sha256").update(buf).digest("hex");

/** Marqueurs : chaînes que le code COMPILÉ conserve (noms de paquets dans les
 *  commentaires/identifiants, classes CSS, API posées sur `window`). Chacun a
 *  été vérifié présent dans un build qui a la capacité, absent dans un build
 *  qui ne l'a pas. */
const MARQUEURS = {
  markdown: ["react-markdown", "remark"],
  katex: ["katex", "KaTeX"],
  fratrie: ["__fratrie"],
  "ws-client": ["subgraph/stream"],
  projection: ["proj-row"],
  // Vérifiés le 2026-09-15 : présents dans le build reconvergence, absents du
  // build 3c042268 qui ne portait pas ces vues.
  ramifications: ["ramifications"],
  "admin-comptes": ["peut_administrer", "api/iam/utilisateurs"],
  // Descente fractale : marqueur VÉRIFIÉ discriminant le 2026-09-15 — présent
  // dans le build 3c042268, ABSENT du build f106e190. (« echelle-bar » et
  // « interieurAxe » ne discriminent pas : les deux lignées les portent.)
  "descente-fractale": ["sous-groupes"],
};

const lire = (p) => { try { return readFileSync(p, "latin1"); } catch { return null; } };
const existe = (p) => { try { return statSync(p).isFile(); } catch { return false; } };

// ── Fermeture des assets atteignables depuis index.html ─────────────────────
const html = lire(join(racine, "index.html"));
const nomDepuisRef = (ref) => ref.split("/").pop();
const atteints = new Map(); // nom -> texte
const aVoir = [...html.matchAll(/\/app\/assets\/([A-Za-z0-9._-]+)/g)].map((m) => nomDepuisRef(m[1]));
const vus = new Set();

while (aVoir.length && vus.size < 200) {
  const nom = aVoir.shift();
  if (vus.has(nom) || nom.endsWith(".map")) continue;
  vus.add(nom);
  const p = join(racine, "assets", nom);
  if (!existe(p)) continue;
  const texte = lire(p);
  if (texte === null) continue;
  atteints.set(nom, texte);
  // Un chunk dynamique est cité par son nom de fichier dans le bundle qui
  // l'importe : on suit ces citations pour ne pas manquer un morceau de code.
  for (const m of texte.matchAll(/assets\/([A-Za-z0-9._-]+\.(?:js|css))/g)) {
    const suivant = m[1];
    if (!vus.has(suivant)) aVoir.push(suivant);
  }
}

const capacites = {};
const preuves = {};
for (const [cap, marqueurs] of Object.entries(MARQUEURS)) {
  const ou = [];
  for (const marqueur of marqueurs) {
    for (const [nom, texte] of atteints) if (texte.includes(marqueur)) ou.push(`${marqueur}@${nom}`);
  }
  capacites[cap] = ou.length > 0;
  preuves[cap] = [...new Set(ou)];
}

const servis = [...atteints.keys()].sort().map((nom) => {
  const p = join(racine, "assets", nom);
  return { nom, taille: statSync(p).size, sha256: sha(readFileSync(p)) };
});

console.log(JSON.stringify({
  dir: racine,
  index_html_sha256: sha(Buffer.from(html, "latin1")),
  assets_atteints: [...atteints.keys()].sort(),
  servis,
  capacites,
  preuves,
}, null, 2));
