// ── RÉSONANCE : LES APPELS DE L'ÉCRAN ────────────────────────────────────────
//
// Ce que la résonance est, côté serveur, est déjà écrit et testé (`crates/
// obrain-cli/src/resonance.rs`) : on voit les FACETTES PUBLIQUES d'un autre self
// — les NOMS de ses objectifs et procédures, les mondes qu'il a apportés, le
// volume de sa mémoire — et JAMAIS ses notes ni son état intime. Le partage est
// un geste, et il se referme.
//
// Ce module ne fait que la porter à l'écran. Deux appels, deux sens :
//   · `GET  /api/engine/resonance` — ce que je vois, ce que je partage, et qui
//     existe (les candidats) ;
//   · `POST /api/engine/resonance` — le geste : lier ou délier.
//
// ── CE QUE CHAQUE SENS VEUT DIRE ─────────────────────────────────────────────
// `je_vois`  : à quoi JE suis abonné → je lis leurs facettes.
// `je_partage`: qui peut lire les MIENNES → on doit savoir qui vous lit.
// `candidats` : les selfs qui existent → on ne relie pas des selfs sans savoir
//               lesquels existent, et un nom de self n'est pas une mémoire.
export interface Candidat {
  self: string;
  entrees?: number;
  derniere_ms?: number | null;
  mondes?: string[];
  noeud_dans_synapse?: boolean;
}

/** Une fiche de facettes : ce qu'un lien donne à lire, et ce qu'il exclut. */
export interface Facettes {
  self_qn?: string;
  self?: string;
  mondes?: string[];
  objectifs?: string[];
  procedures?: string[];
  entrees?: number;
  derniere_ms?: number | null;
  /** Les genres JAMAIS partagés (texte des notes, état intime). */
  exclu?: string[];
  prive_exclu?: boolean;
}

export interface VueResonance {
  moi?: string;
  je_vois?: Facettes[];
  je_partage?: (Facettes | { lecteur?: string; self?: string })[];
  candidats?: Candidat[];
  /** Ce qu'un partage donne, et ce qu'il ne donne pas — écrit par le serveur. */
  regle?: string;
  error?: string;
}

async function lire<T>(url: string): Promise<T> {
  const r = await fetch(url);
  const texte = await r.text();
  let d: any;
  try { d = JSON.parse(texte); } catch { throw new Error(texte.slice(0, 200)); }
  if (!r.ok) throw new Error(d?.error || `HTTP ${r.status}`);
  return d as T;
}

/** Ce que je vois, ce que je partage, et qui existe. */
export function fetchResonance(): Promise<VueResonance> {
  return lire<VueResonance>("/api/engine/resonance");
}

/**
 * Le GESTE : lier (ou délier) un lecteur à MES facettes.
 *
 * Le serveur prend le PUBLIEUR de la session, jamais du corps : on ne peut donc
 * pas s'octroyer la lecture du self d'un autre. Ce que l'écran envoie, c'est
 * seulement QUI reçoit le lien — et le sens est celui de l'interface : « je
 * partage avec ».
 */
export async function gesteResonance(
  lecteur: string, delier = false,
): Promise<{ change?: boolean; etat?: string; note?: string; je_partage?: string[]; error?: string }> {
  const r = await fetch("/api/engine/resonance", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ lecteur, delier }),
  });
  const texte = await r.text();
  let d: any;
  try { d = JSON.parse(texte); } catch { throw new Error(texte.slice(0, 200)); }
  if (!r.ok) throw new Error(d?.error || `HTTP ${r.status}`);
  return d;
}

/** Le nom court d'un self (`self:alice` → `alice`) : l'écran parle des gens. */
export const nomSelf = (qn?: string) =>
  (qn || "").replace(/^self:/, "") || "(anonyme)";
