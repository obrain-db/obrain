import { useState } from "react";
import WorldAdd from "./WorldAdd";
import EngineAsk from "./EngineAsk";

/**
 * LE PANNEAU « MONDE » — ajouter un dépôt, et interroger l'engine une fois.
 *
 * ── POURQUOI IL SORT DU CANVAS (2026-09-16) ─────────────────────────────────
 * `WorldAdd` et `EngineAsk` vivaient dans un `ViewPanel` de `CosmosWeave`, donc
 * DANS la scène WebGL. Deux conséquences, déjà mesurées pour le chat :
 *   · leurs re-rendus suivaient ceux de la scène (mouvement, focus, densité,
 *     niveaux) : `WorldAdd` porte un formulaire et un suivi d'avancement qui
 *     interroge le serveur toutes les 2 s — le faire dépendre du rendu 3D est
 *     un contresens ;
 *   · une scène qui échoue à se monter emportait l'ajout de monde avec elle —
 *     or c'est LE geste par lequel un compte neuf obtient sa première base. Le
 *     perdre à cause du canvas serait le pire couplage possible.
 *
 * C'est aussi ce que la demande disait : « les interfaces de chat doivent être
 * libérées de la charge GPU du Canvas » — l'ajout de monde est du même bord.
 *
 * ── IL S'OUVRE PAR LE MÊME MODÈLE QUE LES AUTRES ────────────────────────────
 * Une classe, des règles média, et un bouton à 44 px dans les deux sens. Les
 * styles en ligne ne peuvent pas avoir de règle pour téléphone : c'est la leçon
 * des trois panneaux précédents, et elle est appliquée ici d'emblée.
 */
export function PanneauMonde() {
  const [ouvert, setOuvert] = useState(false);
  return (
    <>
      <button className={"chip monde-bouton" + (ouvert ? " on" : "")}
        aria-expanded={ouvert}
        title="Ajouter un dépôt GitHub, et interroger l'engine"
        onClick={() => setOuvert((v) => !v)}>
        ➕ <span className="monde-bouton-texte">Monde</span>
      </button>

      {ouvert ? (
        <div className="monde-panneau" role="dialog" aria-label="Ajouter un monde">
          <div className="monde-tete">
            <b>Ajouter un monde</b>
            <button className="chip monde-fermer" onClick={() => setOuvert(false)}
              aria-label="Fermer">✕</button>
          </div>
          {/* LE GESTE DE L'AMI : un dépôt GitHub devient une base à lui, enrichie
              (courbure, hubs, index de texte) — voir `worlds.rs`. */}
          <WorldAdd />
          {/* Une question ponctuelle, sans ouvrir la conversation : c'est le même
              engine, mais la réponse ne s'installe pas dans le fil. */}
          <details className="monde-ask">
            <summary>Poser une question ponctuelle (sans conversation)</summary>
            <EngineAsk />
          </details>
        </div>
      ) : null}
    </>
  );
}
