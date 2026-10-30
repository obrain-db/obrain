/**
 * ── TIMELINE HORIZONTALE (MUR D'HORLOGE) D'UNE SESSION + SOUS-AGENTS ─────────
 *
 * Une ligne de temps par agent, empilées, toutes sur le MÊME axe : le `t_ms`
 * (horodatage d'émission en millisecondes, partagé entre le parent et les
 * sous-agents). Les appels d'outils sont des barres qui s'étendent du `t_ms`
 * d'appel au `t_ms` de résultat — la longueur est la VRAIE durée. Les messages
 * sont des blocs (largeur = longueur du texte, aperçu du contenu).
 *
 * Repli : les événements sans `t_ms` (archives antérieures) retombent sur
 * `seq` (ordre logique, non partagé) — dégradé, jamais vide.
 * Clic / tap → détail. Zoom multi-échelle (boutons +/−, molette).
 */
import { useEffect, useMemo, useState } from "react";
import { fetchTimeline, Timeline, TimelineEvent } from "../lib/api";

interface Bar {
  key: string;
  start: number;
  end: number;
  kind: string;
  role?: string;
  label: string;
  preview: string;
  text: string;
  e: TimelineEvent;
}

interface Ligne {
  titre: string;
  bars: Bar[];
}

const APERCU = 44;
const HAUTEUR_LIGNE = 84;
/** px par seconde d'horloge (le curseur de zoom). */
const ZOOM_BASE = 2;

function tOf(e: TimelineEvent): number {
  return e.t_ms && e.t_ms > 0 ? e.t_ms : e.seq * 1000;
}

function faireBars(evenements: TimelineEvent[]): Bar[] {
  const resT = new Map<string, number>();
  for (const e of evenements) {
    if (e.kind === "resultat" && e.callId) resT.set(e.callId, tOf(e));
  }
  const out: Bar[] = [];
  for (const e of evenements) {
    if (e.kind === "resultat") continue;
    const start = tOf(e);
    const fin = e.kind === "appel" && e.callId ? resT.get(e.callId) ?? start : start;
    const texte = e.text || e.args || "";
    out.push({
      key: `${e.kind}-${e.seq}-${e.callId || ""}`,
      start,
      end: Math.max(fin, start + 1),
      kind: e.kind,
      role: e.role,
      label: e.kind === "appel" ? e.name || "outil" : e.role || "msg",
      preview: aplatir(texte).slice(0, APERCU),
      text: texte,
      e,
    });
  }
  return out;
}

export default function TimelineView({ session, store }: { session: string; store?: string }) {
  const [data, setData] = useState<Timeline | null>(null);
  const [erreur, setErreur] = useState<string | null>(null);
  const [zoom, setZoom] = useState(ZOOM_BASE);
  const [sel, setSel] = useState<TimelineEvent | null>(null);

  useEffect(() => {
    const ctl = new AbortController();
    setData(null);
    setErreur(null);
    setSel(null);
    fetchTimeline(session, store, ctl.signal)
      .then(setData)
      .catch((e) => {
        if ((e as Error)?.name !== "AbortError") setErreur(String((e as Error)?.message || e));
      });
    return () => ctl.abort();
  }, [session, store]);

  const lignes = useMemo<Ligne[]>(() => {
    if (!data) return [];
    const out: Ligne[] = [{ titre: data.compte || "fil principal", bars: faireBars(data.evenements) }];
    for (const a of data.agents || []) {
      out.push({ titre: a.name || a.qn.replace(/^subagent:/, "agent "), bars: faireBars(a.evenements) });
    }
    return out;
  }, [data]);

  if (erreur) return <div className="tl-panneau tl-erreur">Timeline indisponible : {erreur}</div>;
  if (!data) return <div className="tl-panneau tl-chargement">chargement du fil…</div>;

  const tous = lignes.flatMap((l) => l.bars);
  const min = tous.length ? Math.min(...tous.map((b) => b.start)) : 0;
  const max = tous.length ? Math.max(...tous.map((b) => b.end)) : 1;
  const nbAppels = tous.filter((b) => b.kind === "appel").length;
  const px = (ms: number) => ((ms - min) * zoom) / 1000;
  const zoomer = (f: number) => setZoom((z) => Math.min(128, Math.max(0.125, z * f)));

  return (
    <div className="tl-panneau">
      <div className="tl-tete">
        <span className="tl-session" title={data.session}>{court(data.session)}</span>
        <span className="tl-totaux">
          {lignes.length} fil{lignes.length > 1 ? "s" : ""} · {nbAppels} appels · {fmtDuree(max - min)}
        </span>
        <span className="tl-zoom">
          <button className="chip" onClick={() => zoomer(0.5)} title="Zoom arrière">−</button>
          <span className="tl-zoom-val">×{zoom} px/s</span>
          <button className="chip" onClick={() => zoomer(2)} title="Zoom avant">＋</button>
        </span>
      </div>

      <div
        className="tl-chart-scroll"
        onWheel={(e) => {
          if (e.deltaY) zoomer(e.deltaY < 0 ? 1.5 : 1 / 1.5);
        }}
      >
        <div className="tl-chart" style={{ width: px(max) + 140, height: lignes.length * HAUTEUR_LIGNE }}>
          {lignes.map((ligne, li) => (
            <div key={li} className="tl-ligne" style={{ top: li * HAUTEUR_LIGNE }}>
              <div className="tl-ligne-titre">{ligne.titre}</div>
              {ligne.bars.map((b) => {
                const estAppel = b.kind === "appel";
                const largeur = estAppel
                  ? Math.max(4, px(b.end) - px(b.start))
                  : Math.max(18, Math.min(180, (b.preview.length + 2) * 3));
                return (
                  <button
                    key={b.key}
                    className={"tl-bar tl-" + b.kind + (b.role ? " tl-" + b.role : "")}
                    style={{
                      left: px(b.start),
                      width: largeur,
                      height: estAppel ? 24 : 18,
                      top: estAppel ? 44 : 22,
                    }}
                    onClick={() => setSel(b.e)}
                    title={`#${b.e.seq} · ${b.label} · ${fmtDuree(b.end - b.start)}`}
                  >
                    {zoom >= 2 && (
                      <span className="tl-bar-label">
                        {estAppel ? b.label : b.role === "user" ? "▸ " + b.preview : b.preview}
                      </span>
                    )}
                  </button>
                );
              })}
            </div>
          ))}
          {/* graduation temporelle */}
          <div className="tl-regle">
            {Array.from({ length: 24 }, (_, i) => (
              <span key={i} className="tl-grad" style={{ left: i * 60 * zoom }}>
                {fmtDuree(i * 60 * 1000)}
              </span>
            ))}
          </div>
        </div>
      </div>

      {sel && (
        <div className="tl-detail">
          <div className="tl-detail-tete">
            <span className="tl-badge">{sel.kind === "appel" ? "🔧 " + (sel.name || "outil") : sel.role}</span>
            <span className="tl-seq">#{sel.seq}</span>
            {sel.tokens && <span className="tl-tokens">in {sel.tokens.in} · out {sel.tokens.out} · cache {sel.tokens.cache}</span>}
            <button className="chip" onClick={() => setSel(null)}>✕</button>
          </div>
          <pre className="tl-texte">{(sel.text || sel.args || "(aucun texte)")}</pre>
        </div>
      )}
    </div>
  );
}

/** Formate une durée en ms → « 1m 23s » / « 4,2s » / « 800ms ». */
function fmtDuree(ms: number): string {
  if (ms >= 60_000) return `${Math.floor(ms / 60_000)}m ${Math.round((ms % 60_000) / 1000)}s`;
  if (ms >= 1000) return `${(ms / 1000).toFixed(1)}s`;
  return `${Math.round(ms)}ms`;
}

function aplatir(s: string): string {
  return s.replace(/\s+/g, " ").trim();
}

function court(qn: string): string {
  const p = qn.split(":");
  return p.length > 1 ? p.slice(0, 2).join(":") + ":…" : qn;
}
