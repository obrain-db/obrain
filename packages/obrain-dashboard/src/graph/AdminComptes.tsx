import { useCallback, useEffect, useState } from "react";

/**
 * ADMINISTRATION DES COMPTES — l'écran de l'admin, par ESPACE nommé.
 *
 * La garde serveur juge chaque geste sur un espace NOMMÉ (jamais un défaut) :
 * l'écran commence donc par choisir l'espace, et chaque appel le porte.
 * Ce que le serveur refuse en réseau (amorcer, émettre un jeton pour autrui)
 * n'apparaît pas ici : la barrière est dans le noyau, l'écran la respecte.
 *
 * Le panneau ne se montre qu'à un compte qui PEUT administrer (`/api/iam/moi`
 * → `peut_administrer`) : pour les autres, il n'existe pas.
 */
interface Compte {
  utilisateur: string;
  statut: string;
  roles?: string[];
  espaces?: string[];
  admin?: boolean;
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

export function AdminComptes() {
  const [droit, setDroit] = useState<{ espaces: string[] } | null>(null);
  const [ouvert, setOuvert] = useState(false);
  const [espace, setEspace] = useState("");
  const [comptes, setComptes] = useState<Compte[]>([]);
  const [msg, setMsg] = useState("");
  const [secret, setSecret] = useState<{ qui: string; mdp: string } | null>(null);
  const [nom, setNom] = useState("");

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

  const recharger = useCallback(() => {
    if (!espace) return;
    fetch(`/api/iam/utilisateurs?espace=${encodeURIComponent(espace)}`)
      .then(async (r) => {
        const d = await r.json();
        if (r.ok) { setComptes(d.comptes ?? []); setMsg(""); }
        else setMsg(d.error || `HTTP ${r.status}`);
      })
      .catch((e) => setMsg(String(e)));
  }, [espace]);
  useEffect(() => { if (ouvert) recharger(); }, [ouvert, recharger]);

  if (!droit) return null;
  const geste = (chemin: string, corps: Record<string, string>, apres?: (d: any) => void) =>
    post(chemin, { espace, ...corps })
      .then((d) => { apres?.(d); recharger(); })
      .catch((e) => setMsg(String(e.message || e)));

  const bo: React.CSSProperties = {
    font: "inherit", fontSize: 11, padding: "2px 8px", borderRadius: 7,
    border: "1px solid rgba(140,150,170,.4)", background: "rgba(140,150,170,.1)",
    color: "inherit", cursor: "pointer",
  };
  return (
    <>
      <button className="chip" title="Administration des comptes" aria-label="Comptes"
        onClick={() => setOuvert(!ouvert)}
        style={{ position: "fixed", left: 10, bottom: 10, zIndex: 60 }}>
        👥
      </button>
      {ouvert ? (
        <div style={{
          position: "fixed", left: 10, bottom: 46, zIndex: 60, width: "min(430px, calc(100vw - 20px))",
          maxHeight: "min(70vh, 560px)", overflowY: "auto", padding: 10, borderRadius: 10,
          background: "rgba(16,19,26,.96)", border: "1px solid rgba(140,150,170,.35)",
          color: "#dfe4ee", fontSize: 12, boxShadow: "0 8px 30px rgba(0,0,0,.45)",
        }}>
          <div style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
            <b>Comptes</b>
            <select value={espace} onChange={(e) => setEspace(e.target.value)}
              style={{ ...bo, cursor: "pointer" }}>
              {droit.espaces.map((e) => <option key={e} value={e}>{e}</option>)}
            </select>
            <button style={bo} onClick={recharger} title="Recharger">↻</button>
            <button style={{ ...bo, marginLeft: "auto" }} onClick={() => setOuvert(false)}>fermer</button>
          </div>
          {msg ? <div style={{ color: "#ff9d7a", marginTop: 6 }}>{msg}</div> : null}
          {secret ? (
            <div style={{ marginTop: 8, padding: 8, border: "1px dashed #d9b95c", borderRadius: 8 }}>
              <b>{secret.qui}</b> — mot de passe (montré UNE seule fois, l'empreinte seule est gardée) :
              <div style={{ fontFamily: "ui-monospace, monospace", userSelect: "all", marginTop: 4 }}>{secret.mdp}</div>
              <button style={{ ...bo, marginTop: 6 }} onClick={() => setSecret(null)}>j'ai noté</button>
            </div>
          ) : null}
          <div style={{ display: "flex", gap: 6, marginTop: 8 }}>
            <input value={nom} onChange={(e) => setNom(e.target.value)} placeholder="nouveau compte…"
              id="admin-nouveau-compte"
              style={{ ...bo, cursor: "text", flex: 1, background: "rgba(0,0,0,.25)" }} />
            <button style={bo} disabled={!nom.trim()}
              onClick={() => geste("/api/iam/utilisateurs", { nom: nom.trim() }, (d) => {
                setSecret({ qui: nom.trim(), mdp: d?.compte?.mot_de_passe ?? "(fourni)" });
                setNom("");
              })}>créer</button>
          </div>
          <table style={{ width: "100%", marginTop: 8, borderCollapse: "collapse" }}>
            <tbody>
              {comptes.map((c) => (
                <tr key={c.utilisateur} style={{ borderTop: "1px solid rgba(140,150,170,.18)" }}>
                  <td style={{ padding: "5px 4px" }}>
                    <b>{c.utilisateur}</b>
                    <span style={{ opacity: 0.6 }}> · {c.statut}{c.roles?.includes("admin") ? " · admin" : ""}</span>
                  </td>
                  <td style={{ textAlign: "right", padding: "4px 0", whiteSpace: "nowrap" }}>
                    {c.statut === "active" ? (
                      <button style={bo} title="Désactiver le compte"
                        onClick={() => geste("/api/iam/utilisateur/desactiver", { nom: c.utilisateur })}>⏻</button>
                    ) : (
                      <button style={bo} title="Réactiver le compte"
                        onClick={() => geste("/api/iam/utilisateur/reactiver", { nom: c.utilisateur })}>▶</button>
                    )}{" "}
                    <button style={bo} title="Nouveau mot de passe (montré une fois)"
                      onClick={() => geste("/api/iam/utilisateur/mot-de-passe", { nom: c.utilisateur }, (d) =>
                        setSecret({ qui: c.utilisateur, mdp: d?.mot_de_passe ?? d?.compte?.mot_de_passe ?? "(voir reponse)" }))}>🔑</button>{" "}
                    <button style={bo} title="Révoquer tous les jetons de session"
                      onClick={() => geste("/api/iam/utilisateur/jetons/revoquer", { nom: c.utilisateur })}>✂</button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          <div style={{ opacity: 0.55, marginTop: 8 }}>
            Amorcer un administrateur et émettre un jeton pour autrui restent HORS BANDE
            (CLI <code>obrain iam</code>) — la barrière est dans le noyau.
          </div>
        </div>
      ) : null}
    </>
  );
}
