import { useCallback, useEffect, useRef, useState } from "react";
import {
  absencesLisible,
  capacitesAnnoncees,
  capacitesHorsCatalogue,
  construireArguments,
  depuisLisible,
  doitBasculerSurRouteHeritee,
  dureeLisible,
  etatDeLaListe,
  ficheDe,
  joignable,
  listeAffichable,
  lireVerdict,
  motifNonJoignable,
  presenceDe,
  resumeVerdict,
  verifierAppel,
  verdictInterface,
  type ClientFlotte,
  type LectureFlotte,
  type RouteFlotte,
  type Verdict,
} from "./flotte";

/**
 * ── LE PANNEAU « MA FLOTTE » — voir ses machines, et leur faire faire
 *    quelque chose, depuis le 8432 ──────────────────────────────────────────
 *
 * ── POURQUOI CE PANNEAU EXISTE ──────────────────────────────────────────────
 * Le canal est en place côté moteur (`remote.rs`, `/api/remote/*`) et le nœud
 * qui s'y rattache est construit (`selfd attacher`). Ce qui manquait était le
 * seul bout : un utilisateur connecté à l'interface ne pouvait NI voir les
 * machines de son compte, NI leur demander quoi que ce soit. Un canal qui n'a
 * pas d'écran n'est pas une capacité, c'est une promesse.
 *
 * ── CE QU'IL CONSOMME, ET RIEN D'AUTRE ──────────────────────────────────────
 *   GET  /api/remote/machines  → LA QUESTION DE LA FLOTTE, RÉPONDUE :
 *                                { compte, question, machines:[…], connues,
 *                                  joignables, aucune, reponse }
 *   GET  /api/remote/clients   → la route HÉRITÉE, en secours SEULEMENT si la
 *                                première rend 404 (binaire antérieur) :
 *                                { compte, clients:[…], aide }
 *   POST /api/remote/appel     → { client_id, outil, arguments, delai_s }
 *                                → { resultat } en 200 ; { error } en 409/400
 *   POST /api/remote/detacher  → { client_id } → { detache: bool }
 *
 * Chaque machine, dans les deux routes de lecture, est le MÊME objet
 * (`Client::en_json` de `remote.rs`) : `id`, `machine`, `capacites`,
 * `attache_ms`, `derniere_vue_ms`, `silence_ms`, `vivant`. Aucun champ n'est
 * inventé ici, et le panneau n'en consomme aucun autre.
 *
 * POURQUOI LA ROUTE DE LA FLOTTE D'ABORD. `/api/remote/clients` LISTE ; elle ne
 * RÉPOND pas à la question « quelles machines connais-tu, et que sait faire
 * chacune ? » : elle ne dit ni combien de machines sont connues, ni combien sont
 * joignables, ni — surtout — qu'il n'y en a aucune. Trois silences différents y
 * rendaient le même corps vide, et l'écran le même carré vide. La route de la
 * flotte rend ces trois choses : des compteurs (`connues`, `joignables`) et une
 * AFFIRMATION (`aucune` + `reponse`). Le panneau s'en sert TELLES QUELLES : il
 * cite la phrase du moteur, il ne la reformule pas, et il confronte les
 * compteurs à la liste reçue (`etatDeLaListe`).
 *
 * Les quatre routes exigent une identité (IAM armé sur le 8432) et sont scopées
 * au compte du jeton : le moteur ne rend QUE vos machines. Le panneau ne
 * fabrique donc aucun en-tête d'authentification — il s'appuie sur la session du
 * navigateur (cookie `obrain_session`), exactement comme `AdminComptes` et
 * `WorldAdd`. Inventer un jeton ici serait écrire un secret dans une page.
 *
 * Charge : `/api/remote/*` ne touche à AUCUN graphe (commentaire en clair dans
 * `viz_serve.rs`, au point de garde). Le sondage au chargement et toutes les 4 s
 * est donc gratuit pour le moteur — c'est ce qui autorise à le faire sans
 * demander à l'utilisateur.
 *
 * ── CE QUE LE PANNEAU REFUSE DE FAIRE ───────────────────────────────────────
 * · Il ne présente JAMAIS une liste vide comme une réponse : « aucune machine »
 *   n'est affiché que si le moteur l'affirme (`aucune: true` + `reponse`) ; tout
 *   autre vide a son propre écran (`vide_non_affirme`, `route_absente`,
 *   `corps_illisible`…), parce qu'une réponse vide ne doit pas avoir la forme
 *   d'une bonne réponse.
 * · Il ne propose JAMAIS un outil qu'une machine n'annonce pas (doctrine
 *   « ANNONCER EST DÉCIDER ») : la liste des outils EST `capacites`, et il DIT
 *   en plus ce que la machine ne fera pas (`absencesLisible`). Le contrôle vit
 *   dans `flotte.ts` (`verifierAppel`) et non dans le rendu, pour être éprouvable
 *   sans navigateur.
 * · Il ne transforme pas un silence en bonne nouvelle : une machine dont le
 *   battement est manqué est affichée « froide », pas « connectée » — et la
 *   mesure du moteur VIEILLIT avec le temps écoulé depuis la lecture (un onglet
 *   en arrière-plan voit ses minuteries étranglées par le navigateur : une liste
 *   vieille de trois minutes ne doit pas dire « vivante »).
 * · Il ne masque aucun refus : le motif rendu par le moteur ou par la politique
 *   locale du nœud est affiché INTÉGRALEMENT, et le corps brut avec lui.
 * · Il n'affirme pas qu'un appel a réussi quand la forme n'est pas reconnue
 *   (« verdict rendu, forme non standard » n'est pas « réussi »).
 *
 * ── OÙ IL SE MONTRE ────────────────────────────────────────────────────────
 * Il n'existe pas dans la barre de commandes ni dans l'inspecteur : ce sont des
 * surfaces du CANVAS, et la flotte n'est pas une propriété du graphe regardé.
 * Il est monté à la racine de l'application, comme `AdminComptes` — un bouton
 * discret (🖥) en bas à gauche qui ouvre un panneau, et qui n'obéit à aucune
 * règle de réduction du chrome : une machine injoignable doit rester atteignable
 * même en chrome réduit, sinon la porte se ferme toute seule.
 *
 * Il s'ouvre aussi par URL : `…/app/?flotte=1` (voir `ouvertureInitiale`), pour
 * qu'on puisse le montrer à quelqu'un sans lui dicter trois clics.
 */

const POLL_MS = 4000;
/** Cadence du vieillissement des présences à l'écran. Cinq secondes : bien plus
 *  fin que les seuils qui DÉCIDENT (30 s pour « vivante », 90 s pour « oubliée »),
 *  et assez rare pour ne rien coûter. Voir `presenceDe(c, ecoule_ms)`. */
const BATTEMENT_MS = 5000;
/** Délais proposés. Le moteur borne à [1, 120] (`api_remote`, `clamp(1, 120)`) :
 *  la liste reste DANS sa borne, sinon le panneau proposerait un délai que le
 *  moteur corrigerait en silence — donc un délai qui n'est pas celui choisi. */
const DELAIS_S = [5, 10, 30, 60, 120];

interface EntreeJournal {
  t: number;
  machine: string;
  outil: string;
  verdict: Verdict;
}

/** Réponse HTTP brute, corps compris : on ne jette jamais le texte reçu. */
interface Reponse {
  statut: number;
  corps: string;
}
const lire = async (r: globalThis.Response): Promise<Reponse> => ({ statut: r.status, corps: await r.text() });
const jsonDe = (r: Reponse): any => {
  try {
    return r.corps ? JSON.parse(r.corps) : null;
  } catch {
    return null;
  }
};

/** Vrai si l'URL de la page demande le panneau ouvert. */
function ouvertureInitiale(): boolean {
  try {
    const p = new URLSearchParams(window.location.search);
    // `?flotte` comme `?flotte=1` : la forme courte est celle qu'on tape.
    return p.has("flotte") && p.get("flotte") !== "0";
  } catch {
    return false;
  }
}

/** Le style des boutons du panneau, partagé par la liste et la ligne exportée. */
const STYLE_BOUTON: React.CSSProperties = {
  font: "inherit", fontSize: 11, padding: "3px 9px", borderRadius: 7,
  border: "1px solid rgba(140,150,170,.4)", background: "rgba(140,150,170,.1)",
  color: "inherit", cursor: "pointer",
};

/** Un point + un libellé : la distinction vivante/froide doit se VOIR d'un
 *  coup d'œil, pas se déduire d'un horodatage qu'il faut lire.
 *
 *  `ecoule_ms` est le temps écoulé DEPUIS LA LECTURE (durée locale). Sans lui,
 *  une liste lue il y a trois minutes continuerait d'afficher « vivante » — voir
 *  `presenceDe` : le moteur mesure à sa réponse, l'écran reste après. */
export function PresenceMachine({ c, ecoule_ms = 0 }: { c: ClientFlotte; ecoule_ms?: number }) {
  const p = presenceDe(c, ecoule_ms);
  return (
    <span style={{ display: "inline-flex", alignItems: "center", gap: 5 }}>
      <span className="dot" style={{ background: p.couleur, boxShadow: `0 0 6px ${p.couleur}` }} />
      <span style={{ color: p.couleur, fontWeight: 600 }}>{p.libelle}</span>
      {p.age_ms !== null ? (
        <span
          style={{ opacity: 0.7 }}
          title={
            `Mesure du moteur : ${dureeLisible(p.mesure_ms ?? p.age_ms)} au moment de sa réponse` +
            (p.ecoule_ms > 0 ? ` ; + ${depuisLisible(p.ecoule_ms)} écoulé depuis la lecture.` : ".") +
            ` Dernière vue : ${new Date(c.derniere_vue_ms).toLocaleString()} (horloge du moteur).`
          }
        >
          · {dureeLisible(p.age_ms)}
        </span>
      ) : null}
    </span>
  );
}

/**
 * ── UNE LIGNE DE LA FLOTTE : ce que l'utilisateur doit pouvoir LIRE ─────────
 *
 * Exportée et PURE (aucun état, aucun réseau) pour une raison de preuve : c'est
 * ici que se joue tout le chantier — « voir ses machines, et distinguer celle qui
 * vit de celle dont on n'a plus de nouvelles ». Un composant qui ne dépend que de
 * ses propriétés peut être RENDU hors navigateur (`react-dom/server`) et son
 * rendu vérifié sur les formes réelles du protocole. Sans cette séparation, la
 * seule preuve possible serait « ça compile », c'est-à-dire rien sur ce que
 * l'utilisateur voit.
 *
 * Les trois informations EXIGÉES par le chantier sont ici, et nulle part
 * ailleurs : le NOM de la machine, ses CAPACITÉS réellement annoncées, et la
 * PRÉSENCE (visiblement distincte d'une machine vivante à une machine froide).
 *
 * Deux ajouts du second passage (2026-09-16) :
 *
 * · *Ce qu'elle NE fera PAS.* Afficher les capacités présentes laissait le reste
 *   dans un blanc, et un blanc se lit « je n'ai pas trouvé le bouton ». La ligne
 *   nomme donc les outils ABSENTS et leur conséquence (`absencesLisible`) — pour
 *   qu'une machine sans `exec` ne laisse pas croire qu'elle peut exécuter.
 * · *La date de rattachement, et non une durée calculée.* Le panneau écrivait
 *   `Date.now() - c.attache_ms` : deux horloges différentes soustraites l'une à
 *   l'autre (celle du navigateur, celle du moteur), exactement ce que `flotte.ts`
 *   interdit par ailleurs. Le champ `attache_ms` est une DATE ; elle s'affiche.
 */
export function LigneMachine({
  c, actif, confirmeDetache, ecoule_ms = 0, onChoisir, onDemanderDetache, onAnnulerDetache, onDetacher,
}: {
  c: ClientFlotte;
  actif: boolean;
  confirmeDetache: boolean;
  /** Temps écoulé depuis la LECTURE de cette liste (durée locale, jamais une
   *  soustraction d'horloges). Voir `PresenceMachine`. */
  ecoule_ms?: number;
  onChoisir: () => void;
  onDemanderDetache: () => void;
  onAnnulerDetache: () => void;
  onDetacher: () => void;
}) {
  const dispo = capacitesAnnoncees(c);
  const p = presenceDe(c, ecoule_ms);
  const absences = absencesLisible(c);
  const horsCatalogue = capacitesHorsCatalogue(c);
  return (
    <div
      style={{
        marginTop: 6, padding: 8, borderRadius: 8, cursor: "pointer",
        border: `1px solid ${actif ? "#7fd1ff" : "rgba(140,150,170,.28)"}`,
        background: actif ? "rgba(23,64,92,.35)" : "rgba(0,0,0,.15)",
      }}
      onClick={onChoisir}
    >
      <div style={{ display: "flex", gap: 6, alignItems: "baseline", flexWrap: "wrap" }}>
        <b style={{ fontSize: 13 }}>{c.machine}</b>
        <PresenceMachine c={c} ecoule_ms={ecoule_ms} />
        <span
          style={{ marginLeft: "auto", opacity: 0.55, fontSize: 10 }}
          title="Date de rattachement, horloge du moteur — affichée, jamais soustraite à l'horloge du navigateur"
        >
          attachée le {new Date(c.attache_ms).toLocaleString()}
        </span>
      </div>
      <div style={{ marginTop: 4, display: "flex", gap: 4, flexWrap: "wrap", alignItems: "center" }}>
        {dispo.length ? (
          dispo.map((n) => (
            <span key={n} style={{
              fontSize: 10, padding: "1px 7px", borderRadius: 999,
              border: `1px solid ${ficheDe(n) ? "rgba(127,209,255,.45)" : "rgba(201,162,39,.55)"}`,
              color: ficheDe(n) ? "#9fdcff" : "#c9a227",
            }} title={ficheDe(n)?.resume || "capacité annoncée par cette machine, sans fiche dans cette version de l'interface : arguments en JSON libre"}>{n}</span>
          ))
        ) : (
          <span style={{ color: "#e0b350", fontSize: 11 }}>
            cette machine n'annonce AUCUNE capacité : rien ne peut lui être demandé
          </span>
        )}
        {horsCatalogue.length ? (
          <span style={{ color: "#c9a227", fontSize: 10 }}>
            ({horsCatalogue.join(", ")} : annoncé(s) sans fiche dans cette version — arguments en JSON libre)
          </span>
        ) : null}
        {p.presence !== "vivante" ? (
          <span style={{ color: p.couleur, fontSize: 11 }}>
            {joignable(c)
              ? "⚠ un appel risque de rendre « délai dépassé »"
              : "⛔ le moteur refuserait la demande"}
          </span>
        ) : null}
      </div>
      {/* ── CE QU'ELLE NE FERA PAS ────────────────────────────────────────────
          « ANNONCER EST DÉCIDER » se lit dans les deux sens : ce qu'une machine
          n'annonce pas est une information, pas un blanc. Chaque absence est
          nommée avec sa conséquence, et la règle qui en découle est répétée —
          aucun bouton ne sera proposé pour un outil non annoncé. */}
      {absences ? (
        <div style={{ marginTop: 4, color: "#e0b350", fontSize: 11 }} data-absences="1">
          {absences}
        </div>
      ) : null}
      <div style={{ marginTop: 5, display: "flex", gap: 6, alignItems: "center" }}>
        <button style={STYLE_BOUTON} onClick={(e) => { e.stopPropagation(); onChoisir(); }}>
          {actif ? "choisie" : "agir sur cette machine"}
        </button>
        {confirmeDetache ? (
          <>
            <span style={{ color: "#e0b350", fontSize: 11 }}>détacher {c.machine} ?</span>
            <button style={STYLE_BOUTON} onClick={(e) => { e.stopPropagation(); onDetacher(); }}>oui</button>
            <button style={STYLE_BOUTON} onClick={(e) => { e.stopPropagation(); onAnnulerDetache(); }}>non</button>
          </>
        ) : (
          <button style={STYLE_BOUTON} title="Le moteur cessera de lui envoyer des demandes"
            onClick={(e) => { e.stopPropagation(); onDemanderDetache(); }}>détacher</button>
        )}
        <span style={{ opacity: 0.4, fontSize: 10, marginLeft: "auto" }} title="Identifiant de rattachement (pas un secret)">
          {c.id}
        </span>
      </div>
    </div>
  );
}

export function PanneauFlotte() {
  const [ouvert, setOuvert] = useState(ouvertureInitiale);
  // La LECTURE (une par sondage), pas la liste : elle porte l'état, la phrase du
  // moteur, ses compteurs et le motif d'un éventuel refus. Voir `etatDeLaListe`.
  const [lecture, setLecture] = useState<LectureFlotte | null>(null);
  // La dernière liste RÉELLEMENT reçue : elle survit à une lecture qui échoue,
  // mais l'écran dit alors qu'elle est périmée (`listeAffichable`).
  const [derniere, setDerniere] = useState<{ machines: ClientFlotte[]; le: number } | null>(null);
  const [erreurTransport, setErreurTransport] = useState<string | null>(null);
  const [lectureLe, setLectureLe] = useState<number | null>(null);
  // Un battement LOCAL, uniquement pour faire vieillir les présences à l'écran :
  // le sondage relit toutes les 4 s, mais un onglet en arrière-plan voit ses
  // minuteries étranglées par le navigateur — sans ce battement, la liste
  // cesserait d'être relue ET cesserait de vieillir, et afficherait « vivante »
  // indéfiniment. C'est le défaut que `presenceDe(c, ecoule_ms)` corrige.
  const [maintenant, setMaintenant] = useState(() => Date.now());
  const [choisi, setChoisi] = useState<string | null>(null);
  const [outil, setOutil] = useState("");
  const [valeurs, setValeurs] = useState<Record<string, string>>({});
  const [jsonLibre, setJsonLibre] = useState("{}");
  const [delai, setDelai] = useState(30);
  const [envoi, setEnvoi] = useState(false);
  const [verdict, setVerdict] = useState<Verdict | null>(null);
  const [journal, setJournal] = useState<EntreeJournal[]>([]);
  const [confirmeDetache, setConfirmeDetache] = useState<string | null>(null);
  const [avisDetache, setAvisDetache] = useState<string | null>(null);
  const envoiCtl = useRef<AbortController | null>(null);

  const vue = listeAffichable(lecture, derniere, lectureLe, erreurTransport !== null);
  const clients: ClientFlotte[] = vue.machines;
  const machine = clients.find((c) => c.id === choisi) ?? null;
  const annoncees = capacitesAnnoncees(machine);
  // L'âge affiché vieillit avec le temps écoulé depuis la lecture DE CETTE LISTE
  // (`vue.le`), et non depuis la dernière tentative : une liste périmée doit
  // vieillir depuis qu'elle a été reçue.
  const ecoule = vue.le !== null ? Math.max(0, maintenant - vue.le) : 0;

  // ── LA LECTURE DE LA FLOTTE ────────────────────────────────────────────────
  const rafraichir = useCallback(async (signal?: AbortSignal) => {
    const lireRoute = async (route: RouteFlotte): Promise<LectureFlotte> => {
      const r = await lire(await fetch(`/api/remote/${route}`, { signal }));
      return etatDeLaListe(r.statut, r.corps, route);
    };
    try {
      let l = await lireRoute("machines");
      // ── LE SEUL REPLI AUTORISÉ, ET IL SE DIT ────────────────────────────────
      // Un 404 signifie que CE binaire ne connaît pas la route de la flotte. On
      // relit alors la route héritée pour ne pas laisser l'utilisateur devant une
      // porte fermée — mais `heritee` reste vrai et l'écran l'annonce, parce que
      // ce chemin-là ne saura jamais affirmer « aucune machine ».
      // Aucun autre état ne bascule : là, la route a parlé, et relire ailleurs
      // masquerait ce qu'elle a dit.
      if (doitBasculerSurRouteHeritee(l)) l = await lireRoute("clients");
      // UN SEUL instant pour toute la lecture : c'est lui qui date la liste ET
      // qui sert à la faire vieillir. Deux `Date.now()` successifs donneraient
      // deux dates pour un même fait, et l'âge affiché en dépendrait.
      const t = Date.now();
      setErreurTransport(null);
      setLecture(l);
      setLectureLe(t);
      setMaintenant(t);
      if (l.etat === "liste" || l.etat === "aucune" || l.etat === "vide_non_affirme") {
        setDerniere({ machines: l.machines, le: t });
      }
    } catch (e) {
      // Annulation volontaire (démontage) : ce n'est pas une erreur à montrer.
      if ((e as Error)?.name === "AbortError") return;
      // Le transport a lâché : aucune route n'a parlé. On ne fabrique pas de
      // lecture (« aucune machine » serait un mensonge) ; on garde la dernière
      // liste connue, marquée périmée.
      setErreurTransport(`moteur injoignable : ${String((e as Error)?.message || e)}`);
    }
  }, []);

  useEffect(() => {
    if (!ouvert) return;
    const ctl = new AbortController();
    void rafraichir(ctl.signal);
    const t = setInterval(() => void rafraichir(ctl.signal), POLL_MS);
    const battement = setInterval(() => setMaintenant(Date.now()), BATTEMENT_MS);
    return () => {
      clearInterval(t);
      clearInterval(battement);
      ctl.abort();
    };
  }, [ouvert, rafraichir]);
  useEffect(() => () => envoiCtl.current?.abort(), []);

  // ── LA SÉLECTION SUIT LA FLOTTE ────────────────────────────────────────────
  // Une machine détachée (ou oubliée par le moteur) disparaît de la liste : si
  // elle restait « choisie », le formulaire d'action viserait un identifiant que
  // le moteur ne connaît plus, et l'utilisateur composerait une demande pour
  // personne. On remet donc la sélection d'aplomb à chaque lecture.
  useEffect(() => {
    if (choisi && !clients.some((c) => c.id === choisi)) {
      setChoisi(null);
      setOutil("");
      setVerdict(null);
    }
    if (!choisi && clients.length === 1) setChoisi(clients[0].id);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [lecture]);

  // ── L'OUTIL CHOISI SUIT LA MACHINE CHOISIE ────────────────────────────────
  // Changer de machine ne doit jamais laisser le formulaire sur un outil que la
  // nouvelle machine n'annonce pas : c'est le même interdit que dans
  // `verifierAppel`, mais vu depuis le rendu — un outil affiché est un outil
  // proposé, et un outil proposé est une promesse.
  useEffect(() => {
    if (!machine) {
      setOutil("");
      return;
    }
    const dispo = capacitesAnnoncees(machine);
    if (!dispo.includes(outil)) setOutil(dispo[0] ?? "");
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [choisi, lecture]);

  const fiche = ficheDe(outil);

  const envoyer = async () => {
    // 1. Bâtir les arguments — par la fiche de l'outil quand elle existe (le
    //    typage se fait là, pour qu'une case mal remplie soit refusée ICI et non
    //    par le nœud avec un motif qui n'apprendrait rien), par le JSON libre
    //    sinon : on n'enferme jamais une machine dans le catalogue d'une version
    //    de l'interface.
    let texte: string;
    if (fiche) {
      const bati = construireArguments(outil, valeurs);
      if (!bati.ok) {
        setVerdict(verdictInterface(bati.motif));
        return;
      }
      texte = JSON.stringify(bati.arguments);
    } else {
      texte = jsonLibre;
    }
    // 2. Le contrôle unique : machine choisie, outil ANNONCÉ, arguments objet.
    //    Il vit dans `flotte.ts` pour être éprouvé sans navigateur.
    const controle = verifierAppel(machine, outil, texte);
    if (!controle.ok) {
      setVerdict(verdictInterface(controle.motif));
      return;
    }
    // 3. « CONNUE N'EST PAS JOIGNABLE » : le registre lui-même dit `vivant:false`
    //    pour une machine relue d'un journal et qui n'a parlé qu'à un processus
    //    mort. Le bouton est déjà désactivé dans ce cas ; ce garde-fou couvre le
    //    cas où la liste aurait vieilli entre l'affichage et le clic.
    const nonJoignable = motifNonJoignable(machine);
    if (nonJoignable) {
      setVerdict(verdictInterface(nonJoignable));
      return;
    }

    setVerdict(null);
    setEnvoi(true);
    envoiCtl.current?.abort();
    const ctl = new AbortController();
    envoiCtl.current = ctl;
    // L'appel est BLOQUANT côté moteur : il attend le verdict du nœud jusqu'à
    // `delai_s` (le handler `api_remote` fait `remote::appeler`, qui attend sur
    // une `Condvar`). On laisse donc au serveur PLUS de temps que le délai
    // qu'on lui a donné (+10 s) : couper à `delai_s` pile ferait disparaître le
    // verdict « délai dépassé » pourtant MOTIVÉ, et le remplacerait par une
    // annulation de transport — deux affichages distincts deviendraient un seul,
    // le moins informatif.
    const minuterie = setTimeout(() => ctl.abort(), (delai + 10) * 1000);
    try {
      const r = await lire(
        await fetch("/api/remote/appel", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ client_id: machine!.id, outil, arguments: controle.arguments, delai_s: delai }),
          signal: ctl.signal,
        }),
      );
      const v = lireVerdict(r.statut, r.corps);
      setVerdict(v);
      setJournal((j) => [{ t: Date.now(), machine: machine!.machine, outil, verdict: v }, ...j].slice(0, 8));
    } catch (e) {
      // Le transport a lâché : le verdict est INCONNU. On le dit, et on ne le
      // présente ni comme un succès ni comme un refus — c'est la seule des
      // quatre issues qui ne vient d'aucune des deux parties.
      const v: Verdict = {
        genre: "transport",
        titre: "Verdict inconnu — transport interrompu",
        detail:
          (e as Error)?.name === "AbortError"
            ? `l'interface a renoncé après ${delai + 10} s sans réponse du moteur. Le moteur a peut-être encore la demande en main : le verdict est INCONNU.`
            : `le réseau a lâché avant tout verdict : ${String((e as Error)?.message || e)} — le verdict est INCONNU.`,
        couleur: "#ff7a5c",
      };
      setVerdict(v);
      setJournal((j) => [{ t: Date.now(), machine: machine!.machine, outil, verdict: v }, ...j].slice(0, 8));
    } finally {
      clearTimeout(minuterie);
      setEnvoi(false);
    }
  };

  const detacher = async (c: ClientFlotte) => {
    setAvisDetache(null);
    try {
      const r = await lire(
        await fetch("/api/remote/detacher", {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ client_id: c.id }),
        }),
      );
      const d = jsonDe(r);
      if (r.statut !== 200) {
        setAvisDetache(`${c.machine} : le moteur a refusé le détachement (HTTP ${r.statut}) — ${d?.error || r.corps.slice(0, 160)}`);
      } else if (d?.detache === true) {
        setAvisDetache(`${c.machine} : détachée (le moteur ne lui enverra plus rien).`);
      } else {
        // `detacher` rend `false` quand le client n'est plus au registre (parti,
        // ou oublié après 90 s de silence). Ce n'est pas un échec : c'est déjà
        // l'état voulu — mais le dire évite de croire à une action sans effet.
        setAvisDetache(`${c.machine} : le moteur ne la connaissait déjà plus (partie, ou oubliée après 90 s de silence).`);
      }
    } catch (e) {
      setAvisDetache(`${c.machine} : détachement impossible — ${String((e as Error)?.message || e)}`);
    } finally {
      setConfirmeDetache(null);
      void rafraichir();
    }
  };

  // Un seul style de bouton pour tout le panneau : `STYLE_BOUTON` sert aussi la
  // ligne exportée (`LigneMachine`). Deux copies finiraient par diverger, et la
  // ligne de la flotte n'aurait plus le même bouton que le reste.
  const bo = STYLE_BOUTON;
  const champ: React.CSSProperties = { ...bo, cursor: "text", background: "rgba(0,0,0,.28)", width: "100%" };
  const titreSection: React.CSSProperties = {
    fontSize: 10, letterSpacing: ".06em", textTransform: "uppercase",
    color: "#64778c", margin: "10px 0 4px",
  };
  const pre: React.CSSProperties = {
    fontFamily: "ui-monospace, monospace", fontSize: 11, whiteSpace: "pre-wrap",
    wordBreak: "break-word", maxHeight: 200, overflow: "auto", userSelect: "all",
    background: "rgba(0,0,0,.3)", borderRadius: 6, padding: 6, margin: "4px 0 0",
  };

  /** Un cadre de texte, un seul, pour que les six états de lecture se
   *  distinguent par leur TITRE et leur couleur — et non par leur forme. */
  const boite = (titre: string, texte: string, couleur: string, pointille = false) => (
    <div
      style={{
        marginTop: 8, padding: 8, borderRadius: 8,
        border: `1px ${pointille ? "dashed" : "solid"} ${couleur}`, background: "rgba(0,0,0,.22)",
      }}
    >
      <div style={{ color: couleur, fontWeight: 600 }}>{titre}</div>
      <div style={{ marginTop: 3, opacity: 0.92 }}>{texte}</div>
    </div>
  );

  const verdictBoite = (v: Verdict) => (
    <div style={{
      marginTop: 8, padding: 8, borderRadius: 8,
      border: `1px solid ${v.couleur}`, background: "rgba(0,0,0,.22)",
    }}>
      <div style={{ color: v.couleur, fontWeight: 600 }}>{v.titre}</div>
      <div style={{ marginTop: 3, opacity: 0.92 }}>{v.detail}</div>
      {v.resultat !== undefined ? <div style={pre}>{JSON.stringify(v.resultat, null, 2)}</div> : null}
      {v.brut && v.resultat === undefined ? <div style={pre}>{v.brut}</div> : null}
    </div>
  );

  return (
    <>
      <button
        className={"chip flotte-bouton" + (ouvert ? " on" : "")}
        title={
          clients.length
            ? `Ma flotte : ${clients.length} machine(s) attachée(s)`
            : "Ma flotte : mes machines (aucune attachée pour l'instant)"
        }
        aria-label="Ma flotte"
        aria-expanded={ouvert}
        onClick={() => setOuvert((v) => !v)}
      >
        🖥{clients.length ? ` ${clients.length}` : ""}
      </button>

      {ouvert ? (
        <div style={{
          position: "fixed", left: 10, bottom: 46, zIndex: 60,
          width: "min(580px, calc(100vw - 20px))", maxHeight: "min(78vh, 660px)", overflowY: "auto",
          padding: 10, borderRadius: 10, background: "rgba(16,19,26,.96)",
          border: "1px solid rgba(140,150,170,.35)", color: "#dfe4ee", fontSize: 12,
          boxShadow: "0 8px 30px rgba(0,0,0,.45)",
        }}>
          <div style={{ display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
            <b>Ma flotte</b>
            <span style={{ opacity: 0.6 }}>
              {lecture?.compte ? `compte ${lecture.compte}` : ""}
            </span>
            <button style={bo} onClick={() => void rafraichir()} title="Relire la liste">↻</button>
            <span style={{ opacity: 0.5, fontSize: 10 }}>
              {lectureLe ? `lu à ${new Date(lectureLe).toLocaleTimeString()}` : "jamais lu"}
              {ecoule >= 1000 ? ` (+${depuisLisible(ecoule)})` : ""}
            </span>
            <button style={{ ...bo, marginLeft: "auto" }} onClick={() => setOuvert(false)}>fermer</button>
          </div>

          <div style={{ opacity: 0.6, marginTop: 4, fontSize: 11 }}>
            Les machines attachées à VOTRE compte — le moteur ne rend que les vôtres.
            Liste relue toutes les {POLL_MS / 1000} s (aucun graphe n'est touché).
          </div>

          {/* ── LA QUESTION, TELLE QUE LE MOTEUR LA FORMULE ───────────────────
              On la cite : elle dit ce que cette liste prétend couvrir — et donc
              ce qu'elle ne couvre pas. Une liste sans question se lit comme une
              réponse complète, quel que soit son contenu. */}
          {lecture?.question ? (
            <div
              style={{ marginTop: 4, fontSize: 10, opacity: 0.55 }}
              title="La question à laquelle cette liste répond, telle que le moteur la formule (`question` de /api/remote/machines)"
            >
              question posée au moteur : « {lecture.question} »
            </div>
          ) : null}

          {/* ── LES COMPTEURS, CONFRONTÉS À LA LISTE ─────────────────────────
              Le moteur déclare (`connues`, `joignables`) ; l'interface compte. Un
              compteur affiché sans vérification serait une affirmation de plus. */}
          {lecture && (lecture.compteurs.connues !== null || lecture.compteurs.joignables !== null) ? (
            <div style={{ marginTop: 4, fontSize: 10, opacity: 0.75 }}>
              compteurs du moteur : {lecture.compteurs.connues ?? "?"} connue(s),{" "}
              {lecture.compteurs.joignables ?? "?"} joignable(s) — listes reçues :{" "}
              {lecture.comptees.total}, joignables comptées :{" "}
              {lecture.comptees.joignables === null ? "non vérifiable" : lecture.comptees.joignables}
              {lecture.accordCompteurs === "verifie" ? " · accordés" : ""}
            </div>
          ) : null}
          {lecture?.motifCompteurs && lecture.accordCompteurs !== "verifie" ? (
            <div
              style={{
                marginTop: 4, fontSize: 11,
                color: lecture.accordCompteurs === "desaccord" ? "#ff7a5c" : "#e0b350",
              }}
            >
              {lecture.accordCompteurs === "desaccord"
                ? "⚠ compteurs du moteur en DÉSACCORD avec sa propre liste : "
                : "· compteurs non vérifiables : "}
              {lecture.motifCompteurs}
            </div>
          ) : null}

          {/* ── LA VOIX DU MOTEUR, OU LE MOTIF DE L'ÉCHEC — JAMAIS UN BLANC ───
              `etatDeLaListe` a rangé la réponse dans un des sept états, et chacun
              a sa boîte : aucune machine (un FAIT, affirmé par le moteur), liste
              vide non affirmée (un DÉFAUT), route absente, corps illisible,
              identité refusée, erreur du moteur. Les confondre rendrait le même
              carré vide pour six causes différentes. */}
          {erreurTransport ? boite("Liste non lue — le moteur n'a pas répondu", erreurTransport, "#ff7a5c") : null}
          {lecture?.etat === "aucune" ? (
            // ── LE VIDE AFFIRMÉ : un FAIT, avec le mode d'emploi du moteur ────
            // `aucune` + `reponse` viennent du moteur : sa phrase est citée MOT
            // POUR MOT (et en police fixe, pour être recopiable) parce qu'elle
            // nomme la commande exacte à lancer. C'est le seul cas où une liste
            // vide a le droit de ressembler à une réponse — parce que le moteur
            // l'a affirmée.
            <div style={{
              marginTop: 8, padding: 8, borderRadius: 8,
              border: "1px dashed rgba(140,150,170,.45)", background: "rgba(0,0,0,.22)",
            }}>
              <div style={{ fontWeight: 600 }}>Aucune machine — le moteur l'AFFIRME (`aucune: true`)</div>
              <div style={{ ...pre, userSelect: "all" }}>{lecture.message}</div>
              <div style={{ marginTop: 4, opacity: 0.9 }}>
                C'est le NŒUD de la machine qui se présente au moteur — pas l'inverse : le
                moteur ne peut pas appeler une machine derrière un routeur. La phrase
                ci-dessus est celle du MOTEUR (`reponse`), pas de cette page. Une fois
                rattachée, la machine apparaîtra ici dans les {POLL_MS / 1000} s.
              </div>
            </div>
          ) : null}
          {lecture && lecture.etat !== "liste" && lecture.etat !== "aucune" ? (
            boite(
              lecture.etat === "vide_non_affirme"
                ? "Liste vide NON affirmée par le moteur"
                : lecture.etat === "route_absente"
                  ? "La flotte n'a pas de route sur ce moteur"
                  : lecture.etat === "corps_illisible"
                    ? "Réponse 200 qui n'est pas la flotte"
                    : lecture.etat === "identite_refusee"
                      ? "Identité refusée — la flotte n'a pas été lue"
                      : "Le moteur a refusé de répondre",
              lecture.message,
              "#ff7a5c",
            )
          ) : null}

          {/* ── LA ROUTE HÉRITÉE SE DIT ───────────────────────────────────────
              Si la route de la flotte a rendu 404, la liste vient de
              `/api/remote/clients` : ce chemin LISTE mais n'affirme ni le vide ni
              les compteurs. Le taire ferait passer une capacité manquante pour une
              flotte sans machine. */}
          {lecture?.heritee ? (
            <div
              style={{
                marginTop: 6, padding: 6, borderRadius: 6, fontSize: 11,
                border: "1px solid #e0b350", background: "rgba(0,0,0,.25)", color: "#e0b350",
              }}
            >
              Liste lue par la route HÉRITÉE `/api/remote/clients` : ce binaire ne connaît pas
              la route de la flotte (`/api/remote/machines` → 404). Elle liste vos machines,
              mais elle ne sait dire NI qu'il n'y en a aucune, NI combien sont joignables.
            </div>
          ) : null}

          {vue.perimee ? boite("Liste périmée — dernière liste connue", `${vue.motif}`, "#e0b350") : null}

          {/* ── LA LISTE ───────────────────────────────────────────────────── */}
          {clients.length ? (
            <div style={titreSection}>
              Machines ({clients.length})
              {vue.perimee && vue.le !== null ? ` — lues à ${new Date(vue.le).toLocaleTimeString()}` : ""}
            </div>
          ) : null}
          {clients.map((c) => (
            <LigneMachine
              key={c.id}
              c={c}
              actif={c.id === choisi}
              confirmeDetache={confirmeDetache === c.id}
              ecoule_ms={ecoule}
              onChoisir={() => { setChoisi(c.id); setVerdict(null); setAvisDetache(null); }}
              onDemanderDetache={() => setConfirmeDetache(c.id)}
              onAnnulerDetache={() => setConfirmeDetache(null)}
              onDetacher={() => void detacher(c)}
            />
          ))}

          {avisDetache ? (
            <div style={{ marginTop: 6, padding: 6, borderRadius: 6, background: "rgba(0,0,0,.25)", border: "1px solid rgba(140,150,170,.35)" }}>
              {avisDetache}
            </div>
          ) : null}

          {/* ── L'ACTION ───────────────────────────────────────────────────── */}
          {machine ? (
            <>
              <div style={titreSection}>Agir sur {machine.machine}</div>
              {/* ── CONNUE N'EST PAS JOIGNABLE ────────────────────────────────
                  Le registre marque `vivant: false` une machine relue d'un
                  journal, qui n'a parlé qu'à un processus mort. On ne présente
                  donc PAS l'action comme disponible : un bouton actif qui ne
                  peut que rendre un refus est une promesse qu'on sait fausse.
                  Le motif est affiché, et le geste possible (rallumer le nœud)
                  est nommé — un refus doit être actionnable. Le bouton n'est
                  désactivé QUE sur un `false` explicite : quand le champ est
                  absent (moteur plus ancien), on laisse le moteur trancher. */}
              {motifNonJoignable(machine) ? (
                <div style={{ marginTop: 6, padding: 8, borderRadius: 8, border: "1px solid #b07aa1", background: "rgba(0,0,0,.22)" }}>
                  <div style={{ color: "#d9a6c9", fontWeight: 600 }}>Machine connue, mais pas joignable</div>
                  <div style={{ marginTop: 3 }}>{motifNonJoignable(machine)}</div>
                </div>
              ) : null}
              <div style={{ display: "flex", gap: 4, flexWrap: "wrap" }}>
                {annoncees.length ? annoncees.map((n) => (
                  <button
                    key={n}
                    className={"chip" + (n === outil ? " on" : "")}
                    style={{ fontSize: 11 }}
                    title={ficheDe(n)?.resume || "capacité annoncée par cette machine (sans fiche dans cette version : arguments en JSON libre)"}
                    onClick={() => { setOutil(n); setVerdict(null); }}
                  >{n}</button>
                )) : (
                  <span style={{ color: "#e0b350" }}>
                    Aucun outil n'est proposé : cette machine n'annonce aucune capacité.
                    L'interface ne propose jamais un outil qu'une machine n'annonce pas.
                  </span>
                )}
              </div>

              {/* ── CE QU'ELLE NE FERA PAS — dans l'ACTION aussi ────────────────
                  La même phrase que sur la ligne, ici au moment de décider : c'est
                  le moment où « exec » manquant compte vraiment. Un blanc à cet
                  endroit-là laisserait croire qu'il ne reste qu'à trouver le bon
                  bouton, alors que la politique du nœud est fixée à son démarrage
                  et qu'aucune demande ne la desserrera. */}
              {absencesLisible(machine) ? (
                <div style={{ marginTop: 5, color: "#e0b350", fontSize: 11 }}>{absencesLisible(machine)}</div>
              ) : null}

              {outil ? (
                <div style={{ marginTop: 6 }}>
                  <div style={{ opacity: 0.7, fontSize: 11 }}>{fiche?.resume || "outil annoncé, sans fiche d'arguments dans cette version de l'interface"}</div>

                  {fiche ? (
                    fiche.champs.length ? fiche.champs.map((ch) => (
                      <div key={ch.nom} style={{ marginTop: 5 }}>
                        <label style={{ fontSize: 11, opacity: 0.85 }}>
                          {ch.nom}{ch.requis ? " *" : ""}
                          {ch.aide ? <span style={{ opacity: 0.6 }}> — {ch.aide}</span> : null}
                        </label>
                        {ch.type === "booleen" ? (
                          <select
                            value={valeurs[ch.nom] ?? "false"}
                            onChange={(e) => setValeurs((v) => ({ ...v, [ch.nom]: e.target.value }))}
                            style={{ ...bo, display: "block", marginTop: 2 }}
                          >
                            <option value="false">non (défaut)</option>
                            <option value="true">oui</option>
                          </select>
                        ) : (
                          <input
                            value={valeurs[ch.nom] ?? ""}
                            placeholder={ch.type === "liste" ? "un argument par ligne" : ""}
                            onChange={(e) => setValeurs((v) => ({ ...v, [ch.nom]: e.target.value }))}
                            style={{ ...champ, marginTop: 2 }}
                          />
                        )}
                      </div>
                    )) : (
                      <div style={{ marginTop: 4, opacity: 0.7, fontSize: 11 }}>cet outil ne prend aucun argument</div>
                    )
                  ) : (
                    <div style={{ marginTop: 5 }}>
                      <label style={{ fontSize: 11, opacity: 0.85 }}>
                        arguments (objet JSON — cette version de l'interface n'a pas de fiche pour « {outil} »)
                      </label>
                      <textarea
                        value={jsonLibre}
                        onChange={(e) => setJsonLibre(e.target.value)}
                        rows={3}
                        style={{ ...champ, marginTop: 2, fontFamily: "ui-monospace, monospace" }}
                      />
                    </div>
                  )}

                  <div style={{ marginTop: 7, display: "flex", gap: 6, alignItems: "center", flexWrap: "wrap" }}>
                    <label style={{ fontSize: 11, opacity: 0.85 }}>délai</label>
                    <select value={String(delai)} onChange={(e) => setDelai(Number(e.target.value))} style={{ ...bo, cursor: "pointer" }}>
                      {DELAIS_S.map((s) => <option key={s} value={String(s)}>{s} s</option>)}
                    </select>
                    <button
                      className="btn btn-primary"
                      style={{ fontSize: 12, padding: "5px 14px" }}
                      disabled={envoi || !joignable(machine) || !annoncees.includes(outil)}
                      title={
                        motifNonJoignable(machine) ||
                        (!annoncees.includes(outil)
                          ? `« ${outil} » n'est pas annoncé par ${machine.machine} : l'interface ne l'enverra pas`
                          : "Envoyer la demande à cette machine")
                      }
                      onClick={() => void envoyer()}
                    >
                      {envoi ? `en attente du verdict (max ${delai} s)…` : "envoyer"}
                    </button>
                    <span style={{ opacity: 0.55, fontSize: 10 }}>
                      demande adressée à {machine.machine}, exécutée par SA politique locale
                    </span>
                  </div>
                </div>
              ) : null}
            </>
          ) : clients.length ? (
            <div style={{ marginTop: 8, opacity: 0.7 }}>
              Choisissez une machine ci-dessus pour lui demander quelque chose.
            </div>
          ) : null}

          {verdict ? verdictBoite(verdict) : null}

          {journal.length ? (
            <>
              <div style={titreSection}>Journal de cette session ({journal.length})</div>
              <div style={{ opacity: 0.85 }}>
                {journal.map((e, i) => (
                  <div key={i} style={{ borderTop: "1px solid rgba(140,150,170,.18)", padding: "4px 0" }}>
                    <span style={{ opacity: 0.55, fontSize: 10 }}>
                      {new Date(e.t).toLocaleTimeString()} · {e.machine} · {e.outil}
                    </span>{" "}
                    <span style={{ color: e.verdict.couleur }}>{e.verdict.titre}</span>
                    <div style={{ opacity: 0.7, fontSize: 11 }}>{resumeVerdict(e.verdict)}</div>
                  </div>
                ))}
              </div>
            </>
          ) : null}

          <div style={{ opacity: 0.5, marginTop: 8, fontSize: 10 }}>
            Ce panneau ne redémarre rien et n'écrit dans aucune base : il lit la liste des
            machines de votre compte (`GET /api/remote/machines`, celle qui RÉPOND à la question
            de la flotte) et transmet vos demandes. La liste vient du registre du moteur — une
            machine s'y rattache d'elle-même, et son ÉTAT est celui que le moteur annonce
            (`vivant` quand il le fournit) : c'est lui qui décide de la joignabilité, jamais
            cette page. La mesure de présence vieillit avec le temps écoulé depuis la lecture
            (un onglet en arrière-plan est ralenti par le navigateur : une liste de trois
            minutes ne dit pas « vivante »).
          </div>
        </div>
      ) : null}
    </>
  );
}

export default PanneauFlotte;
