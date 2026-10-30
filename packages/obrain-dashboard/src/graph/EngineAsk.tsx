import { useState } from "react";
import { useAtom } from "jotai";
import { subjectAtom } from "../app/store";
import { askEngine, isAbort, type EngineAnswer } from "../lib/api";
import { MarkdownRiche } from "./RenduRiche";

/**
 * DEMANDER À L'ENGINE — un LLM qui raisonne sur CE QUE VOUS VISEZ.
 *
 * Le contexte initial n'est pas saisi : il vient du SUJET partagé (la pièce).
 * C'est le point (3) de l'objectif : ce que l'utilisateur voit ou active
 * alimente le raisonnement. Sans sujet visé, on ne demande rien — et on le dit,
 * plutôt que d'envoyer un contexte vide et de récupérer une réponse plausible.
 *
 * On montre AUSSI le contexte réellement envoyé : une réponse de LLM dont on ne
 * peut pas inspecter les entrées n'est pas vérifiable.
 */
export default function EngineAsk() {
  const [subject] = useAtom(subjectAtom);
  const [question, setQuestion] = useState("");
  const [etat, setEtat] = useState<"repos" | "en_cours" | "fini" | "erreur">("repos");
  const [answer, setAnswer] = useState<EngineAnswer | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [voirContexte, setVoirContexte] = useState(false);

  const vise = !!subject.qn;
  const demander = async () => {
    if (!question.trim() || !vise) return;
    setEtat("en_cours"); setErr(null); setAnswer(null);
    try {
      const a = await askEngine(subject.store, subject.qn, question, 400);
      setAnswer(a);
      setEtat(a.error ? "erreur" : "fini");
      if (a.error) setErr(a.error);
    } catch (e) {
      if (!isAbort(e)) { setErr(String((e as Error)?.message || e).slice(0, 240)); setEtat("erreur"); }
    }
  };

  return (
    <div style={{ marginTop: 8, borderTop: "1px solid rgba(127,209,255,.18)", paddingTop: 6 }}>
      <div className="q-intro" style={{ margin: 0 }}>
        <b>Engine</b> — un LLM qui raisonne sur le graphe, à partir de ce que vous visez.
      </div>
      {!vise ? (
        <div className="q-intro" style={{ opacity: 0.7, marginTop: 2 }}>
          Visez d'abord un monde ou un nœud (←/→ puis ↓, ou un clic) : c'est lui qui
          deviendra le contexte.
        </div>
      ) : (
        <>
          <div className="q-intro" style={{ marginTop: 2, opacity: 0.85 }}>
            sujet : <b>{subject.label || subject.qn}</b>
            {subject.via ? <span style={{ opacity: 0.6 }}> (posé par {subject.via})</span> : null}
          </div>
          <div style={{ display: "flex", gap: 6, marginTop: 4 }}>
            <input
              value={question}
              onChange={(e) => setQuestion(e.target.value)}
              onKeyDown={(e) => { if (e.key === "Enter") demander(); }}
              placeholder="Que veux-tu savoir sur ce sujet ?"
              aria-label="Question à l'engine"
              style={{
                flex: "1 1 auto", minWidth: 0, background: "rgba(10,14,19,.8)",
                border: "1px solid #2a3440", borderRadius: 8, color: "#dfe8f1",
                padding: "5px 8px", fontSize: 12,
              }}
            />
            <button className="chip on" onClick={demander} disabled={etat === "en_cours" || !question.trim()}
              title="Interroger l'engine sur le sujet visé">
              {etat === "en_cours" ? "… réflexion" : "Demander"}
            </button>
          </div>
          {etat === "en_cours" && (
            <div className="q-intro" style={{ marginTop: 4, opacity: 0.8 }}>
              Le modèle lit {""}
              <b>le voisinage réel</b> du sujet et rédige. Un modèle local peut
              prendre une minute : le graphe, lui, s'allume <b>tout de suite</b>.
            </div>
          )}
          {err && <div className="q-intro" style={{ color: "#ff9e8a", marginTop: 4 }}>{err}</div>}
          {answer && !err && (
            <div className="q-intro" style={{ marginTop: 4 }}>
              {/* MÊME RENDU QUE LE CHAT : le markdown et le LaTeX de l'agent ne
                  doivent pas dépendre du panneau qui les affiche. */}
              <MarkdownRiche texte={answer.reponse} />
              <div style={{ opacity: 0.7, marginTop: 4, fontSize: 11 }}>
                {answer.modele.split(".")[0]} · {answer.noeuds_contexte} nœuds +{" "}
                {answer.relations_contexte} relations lus en {answer.ms_moteur} ms ·{" "}
                réponse en {(answer.ms_llm / 1000).toFixed(1)} s
                {answer.depuis_raisonnement ? " (texte issu du raisonnement)" : ""}
                {" · "}
                <button className="chip" onClick={() => setVoirContexte((v) => !v)}>
                  {voirContexte ? "masquer" : "voir"} le contexte
                </button>
              </div>
              {voirContexte && (
                <pre style={{
                  marginTop: 4, maxHeight: 220, overflow: "auto", fontSize: 10.5,
                  background: "rgba(5,7,12,.7)", border: "1px solid #223041",
                  borderRadius: 8, padding: 6, whiteSpace: "pre-wrap",
                }}>{answer.contexte}</pre>
              )}
            </div>
          )}
        </>
      )}
    </div>
  );
}
