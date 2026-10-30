#!/usr/bin/env node
// Garde-fou du CONTRAT de la toile de l'univers (vue Cosmos).
//
// Pourquoi : le rendu 3D n'est pas vérifiable par l'agent (son navigateur
// refuse les IP privées), et un champ manquant ne plante pas — il produit un
// `undefined`, un `NaN`, et une scène blanche ou un panneau à trous. Constaté
// une fois : `counts.anchors_woven` absent de la relecture persistée.
//
// Ce script vérifie sans navigateur :
//   1. tous les champs que lit `CosmosWeave.tsx` existent et sont typés ;
//   2. les maths du composant (masse, orbite, taille, affaissement de la
//      nappe, position des synapses) ne produisent AUCUN NaN ;
//   3. le curseur de densification est exploitable (≥ 2 paliers, monotone) ;
//   4. les fibres sont classables (toile forte vs parenté de structure).
//
// Usage : node scripts/check-weave-contract.mjs [url]
//   défaut : http://192.168.1.103:8432/api/viz/weave
// Sortie : 0 si le contrat tient, 1 sinon.

const URL_ = process.argv[2] || "http://192.168.1.103:8432/api/viz/weave";

const fail = [];
const warn = [];
const has = (o, k, where) => {
  if (o == null || o[k] === undefined || o[k] === null) fail.push(`${where}.${k} manquant`);
  return o?.[k];
};

let d;
try {
  const r = await fetch(URL_);
  if (!r.ok) { console.error(`contrat : HTTP ${r.status} sur ${URL_}`); process.exit(1); }
  d = await r.json();
} catch (e) {
  console.error(`contrat : injoignable (${URL_}) — ${e?.message || e}`);
  process.exit(1);
}

if (d.available !== true && !d.available) {
  console.error(`contrat : toile indisponible — ${d.reason || "raison non fournie"}`);
  process.exit(1);
}

// 1 · champs lus par le composant
has(d, "self", "payload"); has(d.self, "qn", "self");
has(d, "clock", "payload");
for (const k of ["t0", "t1", "sessions"]) has(d.clock, k, "clock");
has(d, "counts", "payload");
for (const k of ["worlds", "anchors_woven", "anchors_persisted", "fibers", "flow_ties"]) {
  has(d.counts, k, "counts");
}
has(d, "densification", "payload"); has(d.densification, "steps", "densification");
if (!Array.isArray(d.foci) || !d.foci.length) fail.push("foci vide (le sélecteur de foyer serait mort)");
(d.foci || []).forEach((f, i) => { has(f, "kind", `foci[${i}]`); has(f, "label", `foci[${i}]`); });

for (const [i, w] of (d.worlds || []).entries()) {
  for (const k of ["qn", "label", "path", "nature", "nodes", "mass", "gravity", "orbit", "slug"]) {
    has(w, k, `worlds[${i}]`);
  }
  if (!["connaissance", "self", "utilisateur"].includes(w.nature)) {
    fail.push(`worlds[${i}].nature inconnue : ${w.nature}`);
  }
}
for (const [i, f] of (d.fibers || []).entries()) {
  for (const k of ["a", "b", "weight", "shared", "bridged", "flows"]) has(f, k, `fibers[${i}]`);
  if (!d.worlds.some((w) => w.qn === f.a) || !d.worlds.some((w) => w.qn === f.b)) {
    fail.push(`fibers[${i}] référence un monde absent`);
  }
}
for (const [i, s] of (d.densification.steps || []).entries()) {
  for (const k of ["t", "fibers", "weight", "ratio"]) has(s, k, `steps[${i}]`);
}

// 2 · maths du composant — un NaN blanchirait la scène
const maxLog = Math.max(...d.worlds.map((w) => Math.log10(1 + (w.nodes || 0))), Number.EPSILON);
const orbitR = (w) => 210 + (1 - Math.min(1, w.gravity ?? 0)) * 430;
for (const w of d.worlds) {
  const vals = {
    masse: Math.log10(1 + (w.nodes || 0)) / maxLog,
    rayon: orbitR(w),
    taille: 9 + 30 * (w.mass ?? 0.3),
    orbit: w.orbit,
    noeuds: w.nodes,
  };
  for (const [n, v] of Object.entries(vals)) {
    if (!Number.isFinite(v)) fail.push(`${w.label} : ${n} = ${v}`);
  }
}
// affaissement de la nappe (échantillon de la grille 46×46 sur 2600 unités)
let maxSag = 0;
for (let x = -1300; x <= 1300; x += 130) {
  for (let z = -1300; z <= 1300; z += 130) {
    let sag = 0;
    for (const w of d.worlds) {
      const wx = orbitR(w), wz = 0;
      const d2 = (x - wx) ** 2 + (z - wz) ** 2;
      sag -= (150 * (w.mass ?? 0.3) + 40) / (1 + d2 / (200 * 200 * (0.5 + (w.mass ?? 0.3))));
    }
    if (!Number.isFinite(sag)) fail.push(`nappe : affaissement NaN en (${x},${z})`);
    else maxSag = Math.max(maxSag, -sag);
  }
}
if (maxSag <= 0) fail.push("nappe : aucun affaissement (le sol resterait plat)");

// 3 · curseur de densification
const st = d.densification.steps || [];
if (st.length < 2) fail.push("densification : moins de 2 paliers (curseur inutilisable)");
let prev = -1, nonMono = 0;
for (const s of st) { if (s.weight < prev) nonMono++; prev = s.weight; }
if (nonMono) fail.push(`densification non monotone (${nonMono} paliers)`);
if (st.length && st[0].weight <= 0) warn.push("premier palier à 0 synapse (début vide)");

// 4 · classement des fibres par le rendu
const strong = d.fibers.filter((f) => f.weight > 0);
const kin = d.fibers.filter((f) => f.weight === 0 && (f.flows || 0) > 0);
if (!strong.length) warn.push("aucune fibre forte (aucune entité partagée ni vécu)");

console.log("ÉCHECS :", fail.length ? fail : "aucun");
console.log("AVERTISSEMENTS :", warn.length ? warn : "aucun");
console.log(
  `mondes=${d.worlds.length} fibres=${d.fibers.length} (fortes=${strong.length}, ` +
  `parenté=${kin.length}) foyers=${d.foci.length} paliers=${st.length}`,
);
console.log(`nappe : affaissement max ${maxSag.toFixed(1)} — aucun NaN`);
console.log(`densification : ${st[0]?.weight ?? 0} → ${st[st.length - 1]?.weight ?? 0} synapses (monotone=${nonMono === 0})`);
console.log(`orbites : ${d.worlds.map((w) => `${w.label.split(" ")[0]}=${orbitR(w).toFixed(0)}`).join(" ")}`);
process.exit(fail.length ? 1 : 0);
