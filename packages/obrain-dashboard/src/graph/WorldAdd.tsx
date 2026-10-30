import { useEffect, useState } from "react";
import { useSetAtom } from "jotai";
import { weaveTokenAtom } from "../app/store";
import { addWorld, fetchWorlds, isAbort, worldJob, type WorldJob, type WorldsList } from "../lib/api";

/**
 * ── LES QUATRE ÉTAPES, DITES DANS L'ORDRE ───────────────────────────────────
 *
 * Défaut mesuré le 2026-09-15 : l'écran ne connaissait que « clone → indexation
 * → prêt ». L'enrichissement (courbure, hubs, trigrammes) n'existait donc PAS
 * dans le récit — et comme il n'existait pas non plus dans le serveur, un monde
 * ajouté arrivait sans relief et l'écran annonçait « monde ajouté : N nœuds ».
 * Maintenant que le serveur enrichit, l'étape doit se VOIR : elle est la plus
 * longue après l'indexation, et c'est elle qui fait qu'un monde ajouté vaut un
 * preset.
 */
const ETAPES = ["clone", "indexation", "enrichissement", "fini"] as const;

function rang(etat: WorldJob["etat"]): number {
  const i = ETAPES.indexOf(etat as (typeof ETAPES)[number]);
  return i < 0 ? 0 : i;
}

/** « 2/4 · indexation — création du store et indexation du dépôt » */
function avancement(job: WorldJob): string {
  return `${rang(job.etat) + 1}/${ETAPES.length} · ${job.etat} — ${job.etape}`;
}

/**
 * L'ENRICHISSEMENT, EN CHIFFRES.
 *
 * On dit ce qui a été MESURÉ (arêtes visitées, courbure maximale, hubs retenus,
 * index construits) et ce qui a ÉCHOUÉ. Un rapport qui ne dit que « enrichi »
 * ne se distingue pas d'un enrichissement partiel — c'est exactement le défaut
 * qu'on corrige, et il serait revenu par le rapport.
 */
function rapportEnrichissement(job: WorldJob): string {
  const bouts: string[] = [];
  if (job.enrich_ms) bouts.push(`enrichi en ${(job.enrich_ms / 1000).toFixed(1)} s`);
  if (job.aretes_visitees) {
    bouts.push(
      `courbure : ${job.aretes_visitees} arête(s) visitée(s)` +
        (job.aretes_curvature ? `, ${job.aretes_curvature} mise(s) à jour` : ", déjà à jour") +
        (job.courbure_max != null ? ` (max |κ| ${job.courbure_max.toFixed(3)})` : ""),
    );
  }
  if (job.hubs != null) bouts.push(`hubs : ${job.hubs} retenu(s)`);
  if (job.hubs_refus) bouts.push(`hubs REFUSÉS : ${job.hubs_refus}`);
  if (job.trigrammes?.length) bouts.push(`index de texte : ${job.trigrammes.join(", ")}`);
  return bouts.join(" · ");
}

/**
 * AJOUTER UN MONDE — un dépôt GitHub devient une base, visible dans le Cosmos.
 *
 * Trois choix visibles dans l'interface :
 *   - on ne demande QUE l'adresse GitHub (le reste est déduit) : c'est la seule
 *     information que l'utilisateur a sous la main ;
 *   - l'avancement est affiché par étapes (clone → indexation → enrichissement →
 *     prêt), parce que l'ensemble prend de quelques secondes à plusieurs
 *     minutes et qu'un bouton qui ne dit rien est un bouton qu'on croit cassé ;
 *   - le monde apparaît **enrichi**, avec le compte rendu de ce qui l'a enrichi.
 */
export default function WorldAdd() {
  const [url, setUrl] = useState("");
  const [job, setJob] = useState<WorldJob | null>(null);
  const [liste, setListe] = useState<WorldsList | null>(null);
  const [erreur, setErreur] = useState<string | null>(null);
  const bumpWeave = useSetAtom(weaveTokenAtom);

  const rafraichir = async () => {
    try { setListe(await fetchWorlds()); } catch { /* silencieux */ }
  };
  useEffect(() => { rafraichir(); }, []);

  // Suivi du travail : on interroge tant qu'il n'est pas terminé.
  useEffect(() => {
    if (!job || job.etat === "fini" || job.etat === "erreur") return;
    let vivant = true;
    const t = setInterval(async () => {
      try {
        const j = await worldJob(job.id);
        if (!vivant) return;
        setJob(j);
        if (j.etat === "fini") { rafraichir(); bumpWeave((n) => n + 1); }
        if (j.etat === "erreur") setErreur(j.erreur || "échec de l'ajout");
      } catch { /* on retentera */ }
    }, 2000);
    return () => { vivant = false; clearInterval(t); };
  }, [job, bumpWeave]);

  const lancer = async () => {
    if (!url.trim()) return;
    setErreur(null); setJob(null);
    try {
      const r = await addWorld(url.trim());
      if (r.error) { setErreur(r.error); return; }
      const j = await worldJob(r.job!);
      setJob(j);
      setUrl("");
    } catch (e) {
      if (!isAbort(e)) setErreur(String((e as Error)?.message || e).slice(0, 220));
    }
  };

  const ajoutes = (liste?.mondes || []).filter((m) => m.ajoute_a_chaud);

  return (
    <div style={{ marginTop: 8, borderTop: "1px solid rgba(127,209,255,.18)", paddingTop: 6 }}>
      <div className="q-intro" style={{ margin: 0 }}>
        <b>Ajouter un monde</b> — un dépôt GitHub devient une base, et apparaît ici.
      </div>
      <div style={{ display: "flex", gap: 6, marginTop: 4 }}>
        <input
          value={url}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => { if (e.key === "Enter") lancer(); }}
          placeholder="https://github.com/propriétaire/dépôt"
          aria-label="Adresse du dépôt GitHub"
          style={{
            flex: "1 1 auto", minWidth: 0, background: "rgba(10,14,19,.8)",
            border: "1px solid #2a3440", borderRadius: 8, color: "#dfe8f1",
            padding: "5px 8px", fontSize: 12,
          }}
        />
        <button className="chip on" onClick={lancer}
          disabled={!url.trim() || (!!job && job.etat !== "fini" && job.etat !== "erreur")}
          title="Cloner et indexer ce dépôt comme nouveau monde">
          {job && job.etat !== "fini" && job.etat !== "erreur" ? "…" : "Ajouter"}
        </button>
      </div>
      {job && job.etat !== "fini" && job.etat !== "erreur" && (
        <div className="q-intro" style={{ marginTop: 4, opacity: 0.85 }}>
          <b>{avancement(job)}</b>
          {job.etat === "enrichissement" && (
            <> — la courbure, les hubs et l'index de texte se construisent : c'est
              cette étape qui donne au monde le même relief que les presets</>
          )}
        </div>
      )}
      {job?.etat === "fini" && (
        <div className="q-intro" style={{ marginTop: 4 }}>
          <b>{job.label}</b> ajouté : {job.noeuds} nœuds, {job.aretes} arêtes en{" "}
          {(((job.fin_ms || 0) - job.debut_ms) / 1000).toFixed(1)} s.{" "}
          {rapportEnrichissement(job) || "enrichissement : aucun compte rendu (à vérifier)"}
          {job.enrich_erreurs?.length ? (
            <span style={{ color: "#ffcf8a" }}>
              {" "}— enrichissement PARTIEL : {job.enrich_erreurs.join(" · ")}
            </span>
          ) : null}
        </div>
      )}
      {erreur && <div className="q-intro" style={{ color: "#ff9e8a", marginTop: 4 }}>{erreur}</div>}
      {ajoutes.length > 0 && (
        <div className="q-intro" style={{ marginTop: 4, fontSize: 11, opacity: 0.85 }}>
          {ajoutes.length} monde(s) ajouté(s) :{" "}
          {/* LE MONDE EST ATTRIBUÉ À SON AUTEUR : un monde ajouté apparaît chez
              tout le monde, donc « qui l'a ajouté » doit être lisible ici. */}
          {ajoutes
            .map((m) => m.label.replace(/ \(.*\)/, "") + (m.proprietaire ? ` (${m.proprietaire})` : ""))
            .join(", ")}
        </div>
      )}
    </div>
  );
}
