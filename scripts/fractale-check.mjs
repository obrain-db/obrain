/**
 * CONTRÔLE DE LA DESCENTE FRACTALE (Atlas) — navigateur réel, mesures réelles.
 *
 * Ce qu'il affirme, et qu'aucune capture d'écran ne peut affirmer :
 *  · après un geste de descente, l'étage courant porte des SOUS-GROUPES RÉELS
 *    (nom + effectif) venus du moteur — pas un découpage inventé ;
 *  · la PROFONDEUR augmente d'un geste à l'autre (la descente est récursive) ;
 *  · les noms d'étage sont des TOKENS valides (sinon le niveau suivant est
 *    refusé en 400 par `/api/viz/space`, ce qui est arrivé avant filtrage).
 *
 * Usage :
 *   node scripts/fractale-check.mjs [url]
 *   PROFONDEUR=3 node scripts/fractale-check.mjs http://192.168.1.103:8432/
 * Chrome local requis (CDP 127.0.0.1:9248) — voir scripts/cosmos-capture.mjs.
 */
const CDP = process.env.CDP_URL || "http://127.0.0.1:9248";
const URL_ = process.argv[2] || "http://127.0.0.1:8443/";
const PROFONDEUR_VISEE = Number(process.env.PROFONDEUR || 3);
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
const j = async (p, init) => { const r = await fetch(CDP + p, init); return JSON.parse(await r.text()); };

const journal = [];
const dire = (...a) => { const l = a.join(" "); journal.push(l); console.log(l); };

let ws = null;
try {
  const tab = await j("/json/new?" + encodeURIComponent(URL_), { method: "PUT" });
  if (!tab?.webSocketDebuggerUrl) { console.log("SKIP — Chrome CDP injoignable sur " + CDP); process.exit(0); }
  ws = new WebSocket(tab.webSocketDebuggerUrl);
  let id = 0; const pend = new Map(); const reqs = []; const errs = [];
  const send = (m, p = {}) => new Promise((res) => { const i = ++id; pend.set(i, res); ws.send(JSON.stringify({ id: i, method: m, params: p })); });
  ws.addEventListener("message", (ev) => {
    const m = JSON.parse(ev.data);
    if (m.id && pend.has(m.id)) { pend.get(m.id)(m.result); pend.delete(m.id); return; }
    if (m.method === "Network.requestWillBeSent" && m.params.request.url.includes("/api/viz/")) {
      reqs.push(m.params.request.url.replace(/^https?:\/\/[^/]+/, ""));
    }
    if (m.method === "Runtime.exceptionThrown") {
      errs.push(m.params.exceptionDetails.exception?.description?.split("\n")[0] || m.params.exceptionDetails.text);
    }
  });
  await new Promise((r) => ws.addEventListener("open", r, { once: true }));
  await send("Page.enable"); await send("Runtime.enable"); await send("Network.enable");
  const ev1 = async (e) => (await send("Runtime.evaluate", { expression: e, returnByValue: true })).result?.value;
  const etat = async () => {
    const brut = await ev1("JSON.stringify(window.__fractale || null)");
    try { return brut ? JSON.parse(brut) : null; } catch { return null; }
  };
  const clavier = async (key, keyCode) => {
    await send("Input.dispatchKeyEvent", { type: "rawKeyDown", key, code: key, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode });
    await send("Input.dispatchKeyEvent", { type: "keyUp", key, code: key, windowsVirtualKeyCode: keyCode, nativeVirtualKeyCode: keyCode });
  };
  const molette = async (n, pas = 80) => {
    for (let i = 0; i < n; i++) {
      await send("Input.dispatchMouseEvent", { type: "mouseWheel", x: 720, y: 450, deltaX: 0, deltaY: -110, button: "none" });
      await sleep(pas);
    }
  };

  await sleep(Number(process.env.WAIT_MS || 15000));
  dire("état initial        :", JSON.stringify(await etat()));

  // 1) viser une base : ←/→ parcourt les bases, ↓ y entre.
  await clavier("ArrowRight", 39);
  await sleep(2500);
  await clavier("ArrowDown", 40);

  // 2) descendre : un geste ↓ (le plus gros sous-groupe) puis la molette, tant
  //    que la profondeur augmente et que le budget de temps le permet.
  let maxi = 0; let etageMax = null;
  for (let manche = 1; manche <= PROFONDEUR_VISEE + 2; manche++) {
    await sleep(2200);
    const o = await etat();
    await molette(12);
    await sleep(2400);
    const apres = (await etat()) || o;
    if (apres?.etage && (apres.etage.profondeur ?? 0) >= maxi) { maxi = apres.etage.profondeur; etageMax = apres; }
    dire(`manche ${manche} (p=${apres?.etage?.profondeur ?? "-"}) :`, JSON.stringify({
      etage: apres?.etage?.nom, count: apres?.etage?.count, chemin: apres?.chemin,
      groupes: (apres?.groupes || []).slice(0, 5).map((g) => `${g.nom}(${g.count})`),
    }));
    if ((apres?.etage?.profondeur ?? 0) >= PROFONDEUR_VISEE) break;
    await clavier("ArrowDown", 40);   // entrer dans le plus gros sous-groupe
  }

  const fin = await etat();
  const tokensValides = (fin?.groupes || []).every((g) => /^[A-Za-z0-9._@+%-]+$/.test(g.nom));
  const requetes = [...new Set(reqs)].filter((u) => /space|modules|subgraph/.test(u));
  const quatreCents = reqs.filter((u) => /space/.test(u)).length > 0;

  dire("\n=== VERDICT ===");
  dire(`profondeur atteinte : ${maxi} (visée ${PROFONDEUR_VISEE})`);
  dire(`étage le plus bas  : ${etageMax?.etage?.nom ?? "—"} · ${(etageMax?.etage?.count ?? 0).toLocaleString?.("fr-FR") ?? etageMax?.etage?.count} nœuds`);
  dire(`chemin              : ${(etageMax?.chemin || []).join(" › ") || "—"}`);
  dire(`sous-groupes réels  : ${(fin?.groupes || []).slice(0, 6).map((g) => `${g.nom}(${g.count})`).join(", ") || "—"}`);
  dire(`tokens valides      : ${tokensValides ? "oui" : "NON — un niveau sera refusé en 400"}`);
  dire(`requêtes de descente: ${requetes.join(" | ") || "AUCUNE"}${quatreCents ? "" : "  ⚠ aucune requête /space"}`);
  if (errs.length) dire("exceptions          :", errs.slice(0, 3).join(" · "));

  const ok = maxi >= PROFONDEUR_VISEE && (fin?.groupes?.length ?? 0) >= 2 && tokensValides;
  dire(ok ? "OK — la descente fractale a lieu (mesuré)" : "ÉCHEC — la descente ne dépasse pas la profondeur visée");
  process.exit(ok ? 0 : 1);
} catch (e) {
  console.log("ERREUR du contrôle :", String(e && e.message || e));
  console.log(journal.join("\n"));
  process.exit(2);
} finally {
  try { ws?.close(); } catch { /* rien */ }
}
