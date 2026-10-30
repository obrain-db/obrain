import { useCallback, useEffect, useState } from "react";
import {
  fetchResonance, gesteResonance, nomSelf,
  type Facettes, type VueResonance,
} from "../lib/resonance";

/**
 * LE PANNEAU DES GENS — comptes et résonance.
 *
 * ── POURQUOI ILS SONT ENSEMBLE ───────────────────────────────────────────────
 * Un compte neuf n'a rien à lire : la première chose utile qu'on fait pour
 * quelqu'un, c'est lui ouvrir un espace, puis lui donner à voir quelqu'un. Ces
 * deux gestes se suivent dans le temps et se comprennent l'un par l'autre ;
 * les séparer en deux écrans aurait ajouté un bouton et une recherche.
 *
 * ── CE QUI A CHANGÉ LE 2026-09-15 ────────────────────────────────────────────
 * L'ancien panneau « Comptes » était posé en `position: fixed; left: 10px;
 * bottom: 10px` avec des styles EN LIGNE, sans une seule règle pour téléphone :
 * mesuré, il tombait sur le champ de saisie du chat (recouvrement 274×50 px) et
 * ses boutons faisaient 22 px de haut — sous le seuil tactile. Deux conséquences
 * dans ce fichier : les styles passent en CSS (classe `gens-*`), donc ils peuvent
 * avoir des règles média ; et le panneau se retire pendant la conversation au
 * lieu de la recouvrir.
 *
 * ── LA RÈGLE DE LA RÉSONANCE, DITE À L'ÉCRAN ─────────────────────────────────
 * On montre ce qui devient lisible ET ce qui ne le devient jamais. Un partage
 * dont on ne connaît pas les limites n'est pas un consentement : l'utilisateur
 * découvrirait après coup que ses notes sont sorties — ou croirait le contraire.
 */
interface Compte {
  utilisateur: string;
  statut: string;
  roles?: string[];
  espaces?: string[];
}

const post = async (chemin: string, corps: Record<string, string>) => {
  const r = await fetch(chemin, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams(corps).toString(),
  });
  const texte = await r.text();
  let d: any = {};
  try { d = JSON.parse(texte); } catch { d = { error: texte.slice(0, 200) }; }
  if (!r.ok) throw new Error(d.error || `HTTP ${r.status}`);
  return d;
};

export function PanneauGens() {
  const [ouvert, setOuvert] = useState(false);
  const [onglet, setOnglet] = useState<"resonance" | "comptes">("resonance");

  // ── réservé à qui peut administrer des comptes (l'onglet disparaît sinon) ──
  const [droit, setDroit] = useState<{ espaces: string[] } | null>(null);
  const [espace, setEspace] = useState("");
  const [comptes, setComptes] = useState<Compte[]>([]);
  const [secret, setSecret] = useState<{ qui: string; mdp: string } | null>(null);
  const [nom, setNom] = useState("");

  // ── résonance ──────────────────────────────────────────────────────────────
  const [vue, setVue] = useState<VueResonance | null>(null);
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);
  /** Quel self est DÉPLIÉ dans « Je lis » — un seul à la fois, pour rester lisible. */
  const [ouvertSelf, setOuvertSelf] = useState<string | null>(null);

  useEffect(() => {
    fetch("/api/iam/moi").then(async (r) => {
      if (!r.ok) return;
      const d = await r.json();
      if (d?.peut_administrer && d?.compte?.espaces?.length) {
        setDroit({ espaces: d.compte.espaces });
        setEspace(d.compte.espaces[0]);
      }
    }).catch(() => {});
  }, []);

  const lireResonance = useCallback(() => {
    fetchResonance().then((v) => { setVue(v); setMsg(""); })
      .catch((e) => setMsg(String(e.message || e)));
  }, []);

  const rechargerComptes = useCallback(() => {
    if (!espace) return;
    fetch(`/api/iam/utilisateurs?espace=${encodeURIComponent(espace)}`)
      .then(async (r) => {
        const d = await r.json();
        if (r.ok) setComptes(d.comptes ?? []);
        else setMsg(d.error || `HTTP ${r.status}`);
      })
      .catch((e) => setMsg(String(e)));
  }, [espace]);

  useEffect(() => {
    if (!ouvert) return;
    lireResonance();
    if (espace) rechargerComptes();
  }, [ouvert, espace, lireResonance, rechargerComptes]);

  const gesteCompte = (chemin: string, corps: Record<string, string>, apres?: (d: any) => void) =>
    post(chemin, { espace, ...corps })
      .then((d) => { apres?.(d); rechargerComptes(); })
      .catch((e) => setMsg(String(e.message || e)));

  const lier = (lecteur: string, delier: boolean) => {
    setBusy(true);
    gesteResonance(lecteur, delier)
      .then((r) => { lireResonance(); setMsg(r?.note || ""); })
      .catch((e) => setMsg(String(e.message || e)))
      .finally(() => setBusy(false));
  };

  // Les selfs qui existent, moins ceux déjà reliés : on ne propose pas deux fois.
  const dejaPartages = new Set(
    (vue?.je_partage || []).map((f: any) => f?.self_qn || f?.self || f?.lecteur || ""),
  );
  const dejaVus = new Set((vue?.je_vois || []).map((f: Facettes) => f?.self_qn || f?.self || ""));
  const proposables = (vue?.candidats || []).filter(
    (c) => c.self !== vue?.moi && !dejaPartages.has(c.self) && !dejaVus.has(c.self),
  );

  return (
    <>
      <button className={"chip gens-bouton" + (ouvert ? " on" : "")}
        aria-expanded={ouvert}
        title="Comptes et résonance — qui je lis, qui me lit"
        onClick={() => setOuvert((v) => !v)}>
        👥 <span className="gens-bouton-texte">Gens</span>
      </button>

      {ouvert ? (
        <div className="gens-panneau" role="dialog" aria-label="Comptes et résonance">
          <div className="gens-tete">
            <b>Gens</b>
            <div className="gens-onglets">
              <button className={"chip" + (onglet === "resonance" ? " on" : "")}
                onClick={() => setOnglet("resonance")}>résonance</button>
              {droit ? (
                <button className={"chip" + (onglet === "comptes" ? " on" : "")}
                  onClick={() => setOnglet("comptes")}>comptes</button>
              ) : null}
            </div>
            <button className="chip gens-fermer" onClick={() => setOuvert(false)}
              aria-label="Fermer">✕</button>
          </div>

          {msg ? <div className="gens-msg">{msg}</div> : null}

          {onglet === "resonance" ? (
            <div className="gens-corps">
              {/* LA RÈGLE, DITE AVANT LES GESTES. Un consentement dont on ne
                  connaît pas les limites n'en est pas un. */}
              <p className="gens-regle">
                Un lien donne à lire des <b>noms</b> (objectifs, procédures), les
                mondes apportés, et le volume de mémoire. Le <b>texte des notes</b>
                et l'état intime ne sortent <b>jamais</b> — et le lien se referme.
              </p>

              <div className="gens-bloc">
                <div className="gens-titre">
                  Je lis <span className="gens-n">{vue?.je_vois?.length || 0}</span>
                </div>
                {(vue?.je_vois || []).length === 0 ? (
                  <div className="gens-vide">
                    Aucun lien. Personne ne vous montre ses facettes — et vous ne
                    voyez rien de personne.
                  </div>
                ) : (
                  <ul className="gens-liste">
                    {(vue?.je_vois || []).map((f) => {
                      const qui = f.self_qn || f.self || "";
                      const ouvertIci = ouvertSelf === qui;
                      return (
                        <li key={qui}>
                          {/* LA LIGNE EST UN BOUTON, PAS UN TEXTE.
                              Avant, un self qu'on lit n'était qu'un NOM suivi de
                              compteurs : on savait qu'il partageait quelque chose,
                              sans jamais pouvoir le LIRE. Or c'est tout l'objet de
                              la résonance — voir les facettes, pas savoir qu'elles
                              existent. La ligne s'ouvre donc, et dit ce qu'elle a. */}
                          <button className={"gens-ligne" + (ouvertIci ? " on" : "")}
                            aria-expanded={ouvertIci}
                            onClick={() => setOuvertSelf(ouvertIci ? null : qui)}>
                            <b>{nomSelf(qui)}</b>
                            <span className="gens-meta">
                              {f.objectifs?.length ? ` · ${f.objectifs.length} objectif(s)` : ""}
                              {f.procedures?.length ? ` · ${f.procedures.length} procédure(s)` : ""}
                              {f.mondes?.length ? ` · ${f.mondes.length} monde(s)` : ""}
                            </span>
                            <span className="gens-chevron">{ouvertIci ? "▾" : "▸"}</span>
                          </button>
                          {ouvertIci ? (
                            <div className="gens-fiche">
                              {/* CE QU'ON LIT — les NOMS partagés, groupés. */}
                              {f.objectifs?.length ? (
                                <>
                                  <div className="gens-sous-titre">Objectifs</div>
                                  <div className="gens-noms">{f.objectifs.join(" · ")}</div>
                                </>
                              ) : null}
                              {f.procedures?.length ? (
                                <>
                                  <div className="gens-sous-titre">Procédures</div>
                                  <div className="gens-noms">{f.procedures.join(" · ")}</div>
                                </>
                              ) : null}
                              {f.mondes?.length ? (
                                <>
                                  <div className="gens-sous-titre">Mondes apportés</div>
                                  <div className="gens-noms">{f.mondes.join(" · ")}</div>
                                </>
                              ) : null}
                              {!f.objectifs?.length && !f.procedures?.length && !f.mondes?.length ? (
                                <div className="gens-vide">
                                  Ce self n'a encore rien partagé de nommé — seuls le
                                  volume et la date sont lisibles.
                                </div>
                              ) : null}
                              {/* CE QUI N'EST PAS LISIBLE, DIT SUR CHAQUE FICHE.
                                  Un partage dont on ignore les limites se lit comme
                                  un partage total : c'est la fiche qui doit le dire,
                                  pas seulement la règle en tête de panneau. */}
                              <div className="gens-exclu">
                                jamais lisible : {(f.exclu || ["note (texte libre)", "etat-self (intime)"]).join(", ")}
                              </div>
                              <div className="gens-actions">
                                <button className="chip gens-act" disabled={busy}
                                  title="Refermer ce lien : ce self ne verra plus vos facettes"
                                  onClick={() => lier(qui, true)}>refermer de mon côté</button>
                              </div>
                            </div>
                          ) : null}
                        </li>
                      );
                    })}
                  </ul>
                )}
              </div>

              <div className="gens-bloc">
                <div className="gens-titre">
                  Qui me lit <span className="gens-n">{vue?.je_partage?.length || 0}</span>
                </div>
                {(vue?.je_partage || []).length === 0 ? (
                  <div className="gens-vide">Personne. Vos facettes ne sont montrées à aucun self.</div>
                ) : (
                  <ul className="gens-liste">
                    {(vue?.je_partage || []).map((f: any) => {
                      const qui = f?.self_qn || f?.self || f?.lecteur || "";
                      return (
                        <li key={qui}>
                          <b>{nomSelf(qui)}</b>
                          <button className="chip gens-act" disabled={busy}
                            title="Refermer ce lien : cette personne ne verra plus vos facettes"
                            onClick={() => lier(qui, true)}>refermer</button>
                        </li>
                      );
                    })}
                  </ul>
                )}
              </div>

              <div className="gens-bloc">
                <div className="gens-titre">Ouvrir un lien</div>
                {proposables.length === 0 ? (
                  <div className="gens-vide">
                    Aucun autre self à qui ouvrir un lien pour l'instant.
                  </div>
                ) : (
                  <div className="gens-choix">
                    {proposables.map((c) => (
                      <button key={c.self} className="chip" disabled={busy}
                        title={`Montrer mes facettes publiques à ${nomSelf(c.self)}`}
                        onClick={() => lier(c.self, false)}>
                        + {nomSelf(c.self)}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            </div>
          ) : (
            <div className="gens-corps">
              <div className="gens-bloc">
                <div className="gens-titre">
                  Comptes
                  <select className="gens-select" value={espace}
                    onChange={(e) => setEspace(e.target.value)}>
                    {(droit?.espaces || []).map((e) => <option key={e} value={e}>{e}</option>)}
                  </select>
                </div>
                {secret ? (
                  <div className="gens-secret">
                    <b>{secret.qui}</b> — mot de passe, montré <b>une seule fois</b> (seule
                    l'empreinte est gardée) :
                    <div className="gens-mdp">{secret.mdp}</div>
                    <button className="chip" onClick={() => setSecret(null)}>j'ai noté</button>
                  </div>
                ) : null}
                <div className="gens-nouveau">
                  <input className="gens-input" value={nom}
                    onChange={(e) => setNom(e.target.value)}
                    placeholder="nom du nouvel ami…" aria-label="Nom du nouvel ami" />
                  <button className="chip" disabled={!nom.trim()}
                    onClick={() => gesteCompte("/api/iam/utilisateurs", { nom: nom.trim() }, (d) => {
                      setSecret({ qui: nom.trim(), mdp: d?.compte?.mot_de_passe ?? "(fourni)" });
                      setNom("");
                    })}>créer</button>
                </div>
                <ul className="gens-liste">
                  {comptes.map((c) => (
                    <li key={c.utilisateur}>
                      <b>{c.utilisateur}</b>
                      <span className="gens-meta"> · {c.statut}</span>
                      <span className="gens-actions">
                        <button className="chip gens-act"
                          title="Nouveau mot de passe (montré une fois)"
                          onClick={() => gesteCompte("/api/iam/utilisateur/mot-de-passe",
                            { nom: c.utilisateur },
                            (d) => setSecret({ qui: c.utilisateur, mdp: d?.mot_de_passe ?? "(voir réponse)" }))}>🔑</button>
                        <button className="chip gens-act"
                          title="Révoquer toutes ses sessions"
                          onClick={() => gesteCompte("/api/iam/utilisateur/jetons/revoquer", { nom: c.utilisateur })}>✂</button>
                        {c.statut === "active" ? (
                          <button className="chip gens-act" title="Désactiver"
                            onClick={() => gesteCompte("/api/iam/utilisateur/desactiver", { nom: c.utilisateur })}>⏻</button>
                        ) : (
                          <button className="chip gens-act" title="Réactiver"
                            onClick={() => gesteCompte("/api/iam/utilisateur/reactiver", { nom: c.utilisateur })}>▶</button>
                        )}
                      </span>
                    </li>
                  ))}
                </ul>
                <p className="gens-note">
                  Amorcer un administrateur et émettre un jeton pour autrui restent
                  <b> hors bande</b> (CLI <code>obrain iam</code>) : la barrière est
                  dans le noyau, l'écran la respecte.
                </p>
              </div>
            </div>
          )}
        </div>
      ) : null}
    </>
  );
}

/** L'ancien nom, gardé pour les appels existants. */
export const AdminComptes = PanneauGens;
