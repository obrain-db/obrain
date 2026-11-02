//! RÉSONANCE — voir les FACETTES PUBLIQUES d'un autre self, sans rien partager
//! de sa mémoire privée.
//!
//! Demande : « plus interconnecter chacun sa base et une raisonnance commune ça
//! peut être à deux ou plus ». Le piège est dans le mot « interconnecter » : la
//! façon évidente de le faire est de copier des mémoires d'un self dans l'autre,
//! et c'est exactement ce qu'il ne faut pas — un self qui contient les souvenirs
//! d'un autre n'est plus un self, et personne ne peut plus savoir ce qu'il pense
//! vraiment.
//!
//! ── CE QUI DÉCIDE DE CE QUI SE PARTAGE EST DÉJÀ DANS LE MODÈLE ─────────────
//! `memoire::Intention` distingue quatre gestes, et cette distinction porte
//! elle-même la frontière :
//!
//!   · `Note { texte, sujet }` — du **texte libre**. C'est la mémoire privée
//!     par excellence. Elle ne sort JAMAIS.
//!   · `EtatSelf { … note }` — un état intime, avec son propre texte. Idem.
//!   · `Objectif { nom, texte }` et `Procedure { nom, description, corps }` —
//!     des artefacts **nommés**, faits pour être désignés et réutilisés. C'est
//!     ce qu'on partage : leurs NOMS, pas leur prose.
//!
//! On n'invente donc pas une politique de confidentialité : on lit celle que le
//! modèle de données exprime déjà. C'est la seule qui ne dérivera pas.
//!
//! ── LE PARTAGE EST UN GESTE, ET IL EST RÉVOCABLE ───────────────────────────
//! Rien n'est visible par défaut. Un self ne devient lisible qu'après un
//! `lier` explicite, et `delier` le referme. Un partage qui s'active par défaut
//! n'est pas un partage : c'est une fuite qu'on n'a pas annoncée.
//!
//! Sur disque, un fichier PAR PUBLIER (`resonance-<auteur>.json`) : le
//! souscripteur est la CLÉ de sa propre liste, donc on ne peut pas découvrir
//! qui d'autre est abonné — et le fichier se relit à chaud, sans redémarrage.

use std::path::{Path, PathBuf};

/// Qui est abonné aux facettes d'un self.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Abonnement {
    /// Le self autorisé à lire les facettes publiques.
    pub lecteur: String,
    pub depuis_ms: i64,
}

/// Le fichier d'un PUBLIER : les abonnés à SES facettes.
///
/// Indexé par le publieur, pas par le lecteur : chacun ne détient que sa propre
/// liste d'abonnés, et personne ne peut lire celle des autres.
fn chemin(publieur: &str) -> PathBuf {
    let dir = dir();
    let _ = std::fs::create_dir_all(&dir);
    // Le répertoire du consentement est PRIVÉ (0700) : voir `ecrire_prive`.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
    }
    dir.join(format!("resonance-{:016x}.json", empreinte(publieur)))
}

/// Le répertoire des liens (même cache que les autres données de service).
fn dir() -> PathBuf {
    std::env::var_os("OBRAIN_VIZ_CACHE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp/obrain-viz-cache"))
        .join("resonance")
}

/// L'empreinte du nom d'un publieur, qui nomme son fichier.
///
/// Extraite POUR LE TEST : un test qui recalculerait la formule à sa façon
/// mesurerait sa propre copie, pas le chemin réel — et passerait au vert le jour
/// où les deux divergent.
fn empreinte(publieur: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    publieur.hash(&mut h);
    h.finish()
}

/// Lit les abonnés d'un publieur (vide si le fichier n'existe pas encore).
///
/// Rendue PUBLIQUE parce que le catalogue et les routes en ont besoin, et que
/// deux lectures divergentes de la même donnée seraient deux vérités.
#[must_use]
pub fn abonnes_de(publieur: &str) -> Vec<Abonnement> {
    let Ok(txt) = std::fs::read_to_string(chemin(publieur)) else {
        return Vec::new();
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
        return Vec::new();
    };
    // Le fichier porte son publieur : sans cette vérification, un fichier
    // déplacé ou copié servirait les abonnés d'un autre — et un lien deviné
    // relie parfois la mauvaise personne.
    if v.get("publieur").and_then(|x| x.as_str()) != Some(publieur) {
        return Vec::new();
    }
    v.get("abonnes")
        .and_then(|x| x.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|a| {
                    Some(Abonnement {
                        lecteur: a.get("lecteur")?.as_str()?.to_string(),
                        depuis_ms: a.get("depuis_ms").and_then(|x| x.as_i64()).unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// ── QUI ME LIT : LES LIENS VUS DEPUIS LE LECTEUR ───────────────────────────
///
/// `abonnes_de` répond « qui peut lire X ». Il manquait la question inverse, et
/// c'est celle que se pose un self : **à quoi suis-je abonné ?** Elle s'obtient
/// en parcourant les fichiers (un par publieur) — le répertoire est petit, et
/// cette lecture à chaud est ce qui permet à un lien posé par le CLI d'être vu
/// par le service sans redémarrage.
///
/// Rend `(publieur, depuis_ms)` pour chaque lien où `lecteur` figure.
#[must_use]
pub fn abonnements_de(lecteur: &str) -> Vec<(String, i64)> {
    let mut out: Vec<(String, i64)> = Vec::new();
    let Ok(entrees) = std::fs::read_dir(dir()) else {
        return out;
    };
    for e in entrees.flatten() {
        let chemin = e.path();
        let nom = chemin.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !nom.starts_with("resonance-")
            || chemin.extension().and_then(|x| x.to_str()) != Some("json")
        {
            continue;
        }
        let Ok(txt) = std::fs::read_to_string(&chemin) else {
            continue;
        };
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&txt) else {
            continue;
        };
        let Some(publieur) = v.get("publieur").and_then(|x| x.as_str()) else {
            continue;
        };
        let Some(abonnes) = v.get("abonnes").and_then(|x| x.as_array()) else {
            continue;
        };
        for a in abonnes {
            if a.get("lecteur").and_then(|x| x.as_str()) == Some(lecteur) {
                out.push((
                    publieur.to_string(),
                    a.get("depuis_ms").and_then(|x| x.as_i64()).unwrap_or(0),
                ));
            }
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Écrit la liste des abonnés d'un publieur (le fichier est petit, et relu à
/// chaque appel : un lien externe devient visible sans redémarrer le service).
fn ecrire(publieur: &str, abonnes: &[Abonnement]) -> std::io::Result<()> {
    let corps = serde_json::json!({
        "publieur": publieur,
        "maj_ms": crate::activation::now_ms(),
        "abonnes": abonnes,
    })
    .to_string();
    ecrire_prive(&chemin(publieur), &corps)
}

/// ── LE CONSENTEMENT N'EST PAS UN CACHE COMME LES AUTRES ─────────────────────
///
/// Défaut mesuré le 2026-09-15 : le cache tourne sous `umask 0002`, donc
/// `std::fs::write` créait ces fichiers en **664** — lisibles par tout le
/// monde. Pour des sous-graphes ou des agrégats, c'est sans conséquence. Pour
/// QUI PEUT LIRE QUEL SELF, c'est une indiscrétion : la liste des liens dit qui
/// s'intéresse à qui, et c'est exactement le genre de graphe social qu'on ne
/// veut pas publier en clair sur une machine partagée. Le fichier est donc écrit
/// en **0600** (lecture et écriture du propriétaire seul), et le répertoire en
/// **0700**.
///
/// On ne l'applique QU'ICI : toucher au reste du cache changerait le
/// comportement d'autres fonctions qui n'ont rien demandé.
fn ecrire_prive(chemin: &Path, corps: &str) -> std::io::Result<()> {
    use std::io::Write;
    // `mode` n'est respecté qu'à la CRÉATION : un fichier existant garde ses
    // droits. On repasse donc `set_permissions` après, sinon un fichier créé
    // avant ce correctif resterait lisible par tous.
    #[cfg(unix)]
    let mut f = {
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(chemin)?
    };
    #[cfg(not(unix))]
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(chemin)?;
    f.write_all(corps.as_bytes())?;
    f.flush()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(chemin, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Établit un lien : `lecteur` pourra voir les facettes publiques de `publieur`.
///
/// Refusé si les deux selfs sont identiques : un lien d'un self vers lui-même
/// n'apprend rien et masque un nom mal orthographié.
pub fn lier(publieur: &str, lecteur: &str) -> anyhow::Result<bool> {
    let (publieur, lecteur) = (publieur.trim(), lecteur.trim());
    if publieur.is_empty() || lecteur.is_empty() {
        anyhow::bail!("résonance : deux selfs sont requis (publieur et lecteur)");
    }
    if publieur == lecteur {
        anyhow::bail!(
            "résonance : « {publieur} » ne peut pas s'abonner à lui-même — \
             il se lit déjà entièrement"
        );
    }
    let mut abonnes = abonnes_de(publieur);
    if abonnes.iter().any(|a| a.lecteur == lecteur) {
        // Idempotent : ré-établir un lien qui existe ne le duplique pas et ne
        // rafraîchit pas sa date — sinon « depuis quand » deviendrait faux.
        return Ok(false);
    }
    abonnes.push(Abonnement {
        lecteur: lecteur.to_string(),
        depuis_ms: crate::activation::now_ms(),
    });
    ecrire(publieur, &abonnes)?;
    Ok(true)
}

/// Referme un lien. Rend `true` si quelque chose a été retiré.
pub fn delier(publieur: &str, lecteur: &str) -> anyhow::Result<bool> {
    let (publieur, lecteur) = (publieur.trim(), lecteur.trim());
    let mut abonnes = abonnes_de(publieur);
    let avant = abonnes.len();
    abonnes.retain(|a| a.lecteur != lecteur);
    if abonnes.len() == avant {
        return Ok(false);
    }
    ecrire(publieur, &abonnes)?;
    Ok(true)
}

/// Ce qu'un lecteur voit d'un publieur : ses FACETTES PUBLIQUES, jamais sa
/// mémoire privée.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Facettes {
    pub self_qn: String,
    /// Les mondes qu'il a apportés (déjà publics : ce sont des dépôts).
    pub mondes: Vec<String>,
    /// Les NOMS de ses objectifs et de ses procédures — pas leur texte.
    pub objectifs: Vec<String>,
    pub procedures: Vec<String>,
    /// Le volume de sa mémoire et sa dernière activité, sans aucun contenu.
    pub entrees: usize,
    pub derniere_ms: Option<i64>,
    /// Toujours vrai : c'est le contrat de cette structure, et le dire évite
    /// qu'un appelant croie avoir reçu « tout » le self.
    pub prive_exclu: bool,
    /// Ce qui a été refusé à la lecture, nommé — un partage partiel qui se
    /// tairait se lirait comme un partage complet.
    pub exclu: Vec<String>,
}

/// Lit les facettes publiques d'un publieur, sur son journal réel.
#[must_use]
pub fn facettes(publieur: &str, journal: &Path) -> Facettes {
    let mut f = Facettes {
        self_qn: publieur.to_string(),
        prive_exclu: true,
        exclu: vec![
            "note (texte libre)".to_string(),
            "etat-self (intime)".to_string(),
        ],
        ..Default::default()
    };
    let Ok(txt) = std::fs::read_to_string(journal) else {
        // Pas de journal : ce n'est PAS une erreur — un self peut exister sans
        // avoir encore rien structuré. On rend une fiche vide et honnête.
        return f;
    };
    for ligne in txt.lines().filter(|l| !l.trim().is_empty()) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(ligne) else {
            continue;
        };
        f.entrees += 1;
        let t = v.get("t").and_then(|x| x.as_i64()).unwrap_or(0);
        f.derniere_ms = Some(f.derniere_ms.map_or(t, |d| d.max(t)));
        let Some(intention) = v.get("intention") else {
            continue;
        };
        let nom = |k: &str| {
            intention
                .get(k)
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string()
        };
        match intention.get("kind").and_then(|x| x.as_str()).unwrap_or("") {
            "objectif" => {
                let n = nom("nom");
                if !n.is_empty() && !f.objectifs.contains(&n) {
                    f.objectifs.push(n);
                }
            }
            "procedure" => {
                let n = nom("nom");
                if !n.is_empty() && !f.procedures.contains(&n) {
                    f.procedures.push(n);
                }
            }
            // `note` et `etat-self` : comptés dans `entrees`, JAMAIS repris.
            _ => {}
        }
    }
    f.derniere_ms = f.derniere_ms.filter(|d| *d > 0);
    f
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("obrain-res-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("répertoire de test");
        d
    }

    /// ── LE CONTRAT DE CONFIDENTIALITÉ, TENU PAR UN TEST ─────────────────────
    ///
    /// C'est le test qui compte : la seule chose qui ne doit JAMAIS arriver est
    /// qu'un texte privé traverse. On écrit un journal contenant les quatre
    /// genres, et on exige que les noms publics sortent, et que le texte privé
    /// — un secret reconnaissable — n'apparaisse NULLE PART dans les facettes.
    #[test]
    fn le_texte_prive_ne_traverse_jamais() {
        let d = base("prive");
        let secret = "mon-code-de-carte-bleue-4242";
        let j = d.join("j.jsonl");
        std::fs::write(
            &j,
            format!(
                "{{\"t\":10,\"self\":\"self:a\",\"provenance\":\"p\",\"intention\":{{\"kind\":\"note\",\"texte\":\"{secret}\",\"sujet\":\"assurance\"}}}}\n\
                 {{\"t\":20,\"self\":\"self:a\",\"provenance\":\"p\",\"intention\":{{\"kind\":\"objectif\",\"nom\":\"publicer-l-atlas\",\"texte\":\"{secret}\"}}}}\n\
                 {{\"t\":30,\"self\":\"self:a\",\"provenance\":\"p\",\"intention\":{{\"kind\":\"procedure\",\"nom\":\"compter-par-label\",\"description\":\"{secret}\",\"corps\":\"{secret}\"}}}}\n\
                 {{\"t\":40,\"self\":\"self:a\",\"provenance\":\"p\",\"intention\":{{\"kind\":\"etat-self\",\"epithet\":\"v1\",\"objectif\":\"o\",\"prochaine_action\":\"a\",\"note\":\"{secret}\"}}}}\n"
            ),
        )
        .expect("journal");

        let f = facettes("self:a", &j);
        let rendu = serde_json::to_string(&f).expect("sérialisation");
        assert!(
            !rendu.contains(secret),
            "du texte privé a traversé les facettes : {rendu}"
        );
        assert_eq!(f.entrees, 4, "toutes les entrées sont COMPTÉES");
        assert_eq!(f.derniere_ms, Some(40), "la dernière activité est réelle");
        assert_eq!(f.objectifs, vec!["publicer-l-atlas".to_string()]);
        assert_eq!(f.procedures, vec!["compter-par-label".to_string()]);
        assert!(f.prive_exclu);
        assert_eq!(f.exclu.len(), 2, "les genres exclus sont NOMMÉS");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Le partage est un geste : rien n'est visible sans lien, et le lien se
    /// referme. Un partage activé par défaut serait une fuite non annoncée.
    ///
    /// ── LE TEST N'ÉCRIT PAS DANS LA VARIABLE D'ENVIRONNEMENT ────────────────
    /// Première version : elle posait `OBRAIN_VIZ_CACHE_DIR` pour isoler le
    /// répertoire — ce que la crate interdit (`#![deny(unsafe_code)]`, et
    /// `set_var` est `unsafe` depuis Rust 2024). On teste donc le CONTRAT là où
    /// il se décide : le fichier d'un publieur ne rend que SES abonnés, et un
    /// fichier qui porte un autre publieur ne rend rien — c'est la vérification
    /// qui compte (un lien deviné relie parfois la mauvaise personne), et elle
    /// se fait sur un fichier écrit à la main.
    #[test]
    fn un_fichier_ne_rend_que_les_abonnes_de_son_publieur() {
        let d = dir();
        std::fs::create_dir_all(&d).expect("répertoire des liens");
        let f = d.join(format!(
            "resonance-{:016x}.json",
            empreinte("self:test-publieur")
        ));
        let _ = std::fs::remove_file(&f);

        assert!(
            abonnes_de("self:test-publieur").is_empty(),
            "aucun abonné quand le fichier n'existe pas"
        );
        // Un fichier bien formé, pour le BON publieur.
        std::fs::write(
            &f,
            r#"{"publieur":"self:test-publieur","maj_ms":1,"abonnes":[{"lecteur":"self:test-lecteur","depuis_ms":7}]}"#,
        )
        .expect("écriture du lien");
        let ab = abonnes_de("self:test-publieur");
        assert_eq!(ab.len(), 1, "l'abonné déclaré est rendu");
        assert_eq!(ab[0].lecteur, "self:test-lecteur");
        assert_eq!(ab[0].depuis_ms, 7, "la date du lien est conservée");

        // Le MÊME fichier, attribué à un autre publieur : plus rien.
        std::fs::write(
            &f,
            r#"{"publieur":"self:un-autre","maj_ms":1,"abonnes":[{"lecteur":"self:test-lecteur","depuis_ms":7}]}"#,
        )
        .expect("écriture");
        assert!(
            abonnes_de("self:test-publieur").is_empty(),
            "un fichier déplacé ne doit pas servir les abonnés d'un autre"
        );
        let _ = std::fs::remove_file(&f);
    }

    /// Les refus : un lien vers soi-même n'apprend rien et masquerait une faute
    /// de frappe dans un nom.
    #[test]
    fn se_lier_a_soi_meme_est_refuse() {
        assert!(lier("self:a", "self:a").is_err());
        assert!(lier("", "self:b").is_err());
        assert!(lier("self:a", "   ").is_err());
    }

    /// Le cycle complet d'un lien : il se POSE, il ne se duplique pas, il a un
    /// SENS, et il se REFERME. Les noms sont uniques par processus pour ne pas
    /// croiser le fichier d'un autre test.
    #[test]
    fn un_lien_se_pose_se_sensualise_et_se_referme() {
        let p = format!("self:p-{}", std::process::id());
        let l = format!("self:l-{}", std::process::id());
        let f = dir().join(format!("resonance-{:016x}.json", empreinte(&p)));
        let _ = std::fs::remove_file(&f);

        assert!(abonnes_de(&p).is_empty(), "aucun partage par défaut");
        assert!(lier(&p, &l).expect("lier"), "le lien se pose");
        assert_eq!(abonnes_de(&p).len(), 1);
        assert!(
            !lier(&p, &l).expect("re-lier"),
            "ré-établir ne duplique pas et ne rafraîchit pas la date"
        );
        assert_eq!(abonnes_de(&p).len(), 1, "toujours un seul abonné");
        assert!(
            abonnes_de(&l).is_empty(),
            "le lien a un SENS : l inverse est vide"
        );

        // ── LA QUESTION DU LECTEUR : À QUOI SUIS-JE ABONNÉ ? ────────────────
        // C'est la même vérité vue de l'autre côté : si elle divergeait, un self
        // croirait lire quelqu'un qui ne l'a jamais autorisé.
        let vus = abonnements_de(&l);
        assert!(
            vus.iter().any(|(qui, _)| qui == &p),
            "le lecteur doit voir à quoi il est abonné : {vus:?}"
        );
        assert!(
            abonnements_de(&p).is_empty(),
            "le publieur n'est abonné à personne du seul fait qu'il publie"
        );

        assert!(delier(&p, &l).expect("délier"), "le lien se referme");
        assert!(abonnes_de(&p).is_empty(), "plus aucun abonné");
        assert!(
            abonnements_de(&l).iter().all(|(qui, _)| qui != &p),
            "le lecteur ne voit plus le lien refermé"
        );
        assert!(!delier(&p, &l).expect("re-délier"), "délier est idempotent");
        let _ = std::fs::remove_file(&f);
    }

    /// ── LE CONSENTEMENT N'EST PAS PUBLIÉ EN CLAIR ───────────────────────────
    ///
    /// Le cache tourne sous `umask 0002` : `fs::write` créait donc ces fichiers
    /// en 664, c'est-à-dire LISIBLES PAR TOUT LE MONDE. Pour un sous-graphe,
    /// c'est indifférent ; pour « qui s'intéresse à quel self », c'est un graphe
    /// social publié sur une machine partagée. Le test mesure les droits
    /// RÉELLEMENT posés : c'est la seule façon de voir revenir la régression si
    /// quelqu'un remplace l'écriture par un `fs::write` plus simple.
    #[cfg(unix)]
    #[test]
    fn le_fichier_de_consentement_est_prive() {
        use std::os::unix::fs::PermissionsExt;
        let p = format!("self:perm-{}", std::process::id());
        let l = format!("self:permlecteur-{}", std::process::id());
        let f = chemin(&p);
        let _ = std::fs::remove_file(&f);

        assert!(lier(&p, &l).expect("lier"), "le lien se pose");
        let m = std::fs::metadata(&f).expect("le fichier existe");
        let mode = m.permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o600,
            "le fichier des liens doit être privé (0600), il est en {mode:o} — \
             les droits du groupe et du monde ne doivent RIEN pouvoir lire"
        );
        let mdir = std::fs::metadata(dir()).expect("le répertoire existe");
        let mdir_mode = mdir.permissions().mode() & 0o777;
        assert_eq!(
            mdir_mode, 0o700,
            "le répertoire des liens doit être privé (0700)"
        );

        // Un fichier ÉCRIT AVANT ce correctif doit être resserré à la réécriture,
        // sinon la correction ne vaudrait que pour les liens neufs.
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o664))
            .expect("droits larges");
        assert!(delier(&p, &l).expect("délier"));
        let m2 = std::fs::metadata(&f).expect("le fichier existe encore");
        assert_eq!(
            m2.permissions().mode() & 0o777,
            0o600,
            "un fichier existant doit être resserré à la réécriture"
        );
        let _ = std::fs::remove_file(&f);
    }
}
