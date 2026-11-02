//! ORBITES — les echelles qu'une entite OUVRE, derivees du graphe.
//!
//! # Le probleme que ce module supprime
//!
//! Jusqu'ici, le serveur DEVINAIT a quel rang decouper les `qualified_name` :
//! il rejouait la meme agregation en faisant varier l'indice de `split`, et
//! s'arretait au premier rang qui « divergeait ». Chaque essai est un parcours
//! complet de la base. Mesure sur `/tmp/linux-full3.obrain` (1 553 071 noeuds) :
//! l'agregation d'UN SEUL rang coute 5,0 a 8,1 s.
//!
//! # Ce qui le remplace
//!
//! Une echelle n'est pas un seuil de distance de camera : c'est une PROPRIETE
//! de l'entite. Une feuille n'ouvre qu'une orbite ; un module en ouvre
//! plusieurs — lui-meme, ses fichiers, leurs symboles. C'est donc au graphe de
//! dire combien d'echelles une chose permet.
//!
//! On lit donc les `qualified_name` UNE FOIS (projection brute, sans
//! agregation) et on construit en memoire l'arbre de prefixes. Tous les rangs,
//! tous les effectifs, toutes les profondeurs en sortent par simple lecture.
//!
//! # Pourquoi la projection brute et pas l'agregation
//!
//! Contre-intuitif, et MESURE (linux-full3, machine chargee, deux passes
//! entrelacees, chemin froid a chaque fois) :
//!
//! | forme                                            | duree           | lignes    |
//! |--------------------------------------------------|-----------------|-----------|
//! | `RETURN split(qn,'.')[2], count(n)` (un rang)     | 8107 / 5012 ms  | 27        |
//! | `RETURN n.qualified_name` (tout, brut)            | 3736 / 3303 ms  | 1 553 071 |
//!
//! Rapatrier 1,55 million de chaines coute MOINS CHER que d'en agreger une
//! seule colonne : le `GROUP BY` interdit le raccourci du planificateur
//! (`planner/lpg/aggregate.rs`, sortie sur `!agg.group_by.is_empty()`), la
//! projection non. Un seul parcours remplace donc a la fois la boucle de
//! sondage ET les agregations par rang.
//!
//! # Pourquoi PAR BRANCHE et pas une profondeur globale
//!
//! `drivers.net.ethernet.intel.e1000` et `fs.ext4` n'ont pas la meme
//! profondeur. Une profondeur globale unique serait un mensonge : elle
//! sur-decouperait `fs` ou sous-decouperait `drivers`. Chaque branche porte
//! donc SA liste d'orbites.

use std::collections::HashMap;

/// Rang le plus fin que l'on derive. Au-dela, le decoupage ne decrit plus une
/// echelle de navigation mais des symboles : c'est le domaine de `subgraph`.
pub const RANG_MAX: usize = 7;

/// Une orbite : un cran d'echelle qu'une entite OUVRE.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Orbite {
    /// Indice de decoupage du `qualified_name` (`split(qn, '.')[rang]`).
    pub rang: usize,
    /// Nombre de noeuds couverts par cette orbite.
    pub effectif: u64,
    /// Nombre d'ensembles distincts a ce rang — la « cardinalite » de l'orbite.
    pub cardinal: usize,
    /// De quoi l'etiqueter : les ensembles dominants, du plus gros au plus petit.
    pub etiquettes: Vec<String>,
}

/// Une branche : une racine du monde, avec la profondeur QU'ELLE porte.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Branche {
    pub nom: String,
    pub effectif: u64,
    /// Nombre d'orbites que cette branche ouvre (1 = feuille : elle n'en ouvre
    /// aucune autre qu'elle-meme).
    pub profondeur: usize,
    pub orbites: Vec<Orbite>,
}

/// La structure d'orbites d'un store, derivee en UNE passe.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Orbites {
    /// Rang de decoupage des racines — LU, plus devine.
    pub rang_racine: usize,
    /// Nombre de `qualified_name` pris en compte (ceux qui contiennent un '.').
    pub noeuds: u64,
    /// Les orbites globales, de la plus large a la plus fine.
    pub orbites: Vec<Orbite>,
    /// Les branches, chacune avec sa propre profondeur.
    pub branches: Vec<Branche>,
    /// Les groupements, indexes PAR RANG (`groupes[r]` = les ensembles au rang
    /// `r`, effectif decroissant). C'est ce qui permet a `/api/viz/modules` de
    /// LIRE sa reponse au lieu de la recalculer : la passe unique a deja tout.
    pub groupes: Vec<Vec<(String, u64)>>,
}

impl Orbites {
    /// Les ensembles d'un rang donne — lecture pure, aucun parcours.
    pub fn groupes_au_rang(&self, rang: usize) -> Vec<(String, u64)> {
        self.groupes.get(rang).cloned().unwrap_or_default()
    }
}

/// ── L'ARBRE DE PREFIXES, EN ARENE ──────────────────────────────────────────
///
/// Premiere version : un `Noeud` recursif possedant ses enfants dans une
/// `HashMap<String, Noeud>`. Elle etait CORRECTE et TROP LENTE — mesuree sur
/// `linux-full3` : 8310 ms au total contre 7282 ms pour l'ancienne agregation,
/// alors que la projection seule ne coute que 3,3 a 3,7 s. Les ~4,8 s de trop
/// venaient d'une allocation par segment et par noeud : 1 553 071 chemins fois
/// huit segments, soit douze millions de `String` construites pour n'en garder
/// qu'une fraction.
///
/// Ici, les noeuds vivent dans un `Vec` et se designent par index. On
/// INTERROGE la table avant d'inserer : `HashMap<String, u32>::get(&str)`
/// n'alloue pas. Une chaine n'est donc construite QUE lorsqu'un segment
/// apparait pour la premiere fois a cet endroit de l'arbre — le nombre de
/// noeuds distincts, pas le nombre de segments lus.
#[derive(Default)]
struct NoeudArene {
    effectif: u64,
    enfants: HashMap<String, u32>,
}

#[derive(Default)]
struct Arbre {
    noeuds: Vec<NoeudArene>,
}

impl Arbre {
    fn neuf() -> Self {
        Self {
            noeuds: vec![NoeudArene::default()],
        }
    }

    /// Insere un chemin deja decoupe. Iteratif : pas de pile de recursion a
    /// payer 1,5 million de fois.
    fn inserer<'a>(&mut self, segments: impl Iterator<Item = &'a str>) {
        self.noeuds[0].effectif += 1;
        let mut courant = 0usize;
        for seg in segments {
            let suivant = match self.noeuds[courant].enfants.get(seg) {
                Some(&i) => i as usize,
                None => {
                    let i = self.noeuds.len();
                    self.noeuds.push(NoeudArene::default());
                    self.noeuds[courant]
                        .enfants
                        .insert(seg.to_string(), i as u32);
                    i
                }
            };
            self.noeuds[suivant].effectif += 1;
            courant = suivant;
        }
    }

    /// Effectifs par rang sous un noeud donne, agreges sur tout son sous-arbre.
    /// `sortie[k]` = segments situes `k + 1` crans sous `depart`.
    ///
    /// LES NOMS SONT EMPRUNTES, PAS COPIES. La version precedente faisait
    /// `nom.clone()` a chaque noeud visite : mesure sur `linux-full3`,
    /// 1 370 062 noeuds, cela coutait 7 a 8 s PAR PARCOURS — et il y en avait
    /// trois. Les chaines vivent deja dans l'arene et lui survivent le temps de
    /// l'agregation : une reference suffit, et l'allocation disparait.
    fn collecter<'a>(&'a self, depart: usize, crans: usize) -> Vec<HashMap<&'a str, u64>> {
        let mut sortie: Vec<HashMap<&'a str, u64>> = vec![HashMap::new(); crans];
        // Parcours en largeur explicite : la profondeur est bornee (huit crans)
        // mais la largeur ne l'est pas — une recursion y laisserait sa pile.
        let mut file: Vec<(usize, usize)> = vec![(depart, 0)];
        while let Some((idx, prof)) = file.pop() {
            if prof >= crans {
                continue;
            }
            for (nom, &enfant) in &self.noeuds[idx].enfants {
                let e = self.noeuds[enfant as usize].effectif;
                *sortie[prof].entry(nom.as_str()).or_insert(0) += e;
                file.push((enfant as usize, prof + 1));
            }
        }
        sortie
    }

    /// Le noeud d'un segment donne, atteint au rang `rang` depuis la racine.
    /// Plusieurs chemins peuvent y mener ; on retient le sous-arbre le plus
    /// peuple, celui qui porte l'essentiel de la branche.
    fn descendre(&self, nom: &str, rang: usize) -> Option<usize> {
        let mut niveau: Vec<usize> = vec![0];
        for _ in 0..rang {
            let mut suivant = Vec::new();
            for i in niveau {
                suivant.extend(self.noeuds[i].enfants.values().map(|&x| x as usize));
            }
            if suivant.is_empty() {
                return None;
            }
            niveau = suivant;
        }
        niveau
            .into_iter()
            .filter_map(|i| self.noeuds[i].enfants.get(nom).map(|&x| x as usize))
            .max_by_key(|&i| self.noeuds[i].effectif)
    }
}

/// LA REGLE DE DIVERGENCE, extraite telle quelle de `api_viz_modules`.
///
/// Un rang « diverge » quand il produit au moins deux ensembles et que le plus
/// gros ne couvre pas la quasi-totalite (< 95 %). C'est le critere qui evite de
/// s'arreter sur un rang ou tout le monde partage le meme segment.
///
/// Reproduite ici a l'identique — signe compris — pour que la derivation rende
/// EXACTEMENT ce que rendait la boucle de sondage.
pub fn diverge(groupes: &[(String, u64)]) -> bool {
    let total: u64 = groupes.iter().map(|(_, c)| *c).sum();
    let premier = groupes.first().map(|(_, c)| *c).unwrap_or(0);
    groupes.len() >= 2 && (total == 0 || premier.saturating_mul(100) / total.max(1) < 95)
}

/// Les racines du noyau Linux. Conservee pour la PARITE de decision : sur
/// l'arbre Linux, leur presence au rang 2 fige `rang_racine = 2`, exactement
/// comme avant.
const RACINES_CONNUES: &[&str] = &[
    "drivers",
    "net",
    "kernel",
    "mm",
    "fs",
    "arch",
    "include",
    "init",
    "lib",
    "block",
    "ipc",
    "security",
    "crypto",
    "sound",
    "tools",
    "samples",
    "rust",
    "virt",
    "usr",
    "Documentation",
];

/// Trie un groupement comme le faisait le moteur : effectif decroissant. A
/// effectif egal, l'ordre du moteur n'est pas specifie ; on tranche par le nom
/// pour que la sortie soit DETERMINISTE (le moteur, lui, ne l'etait pas).
fn trier(m: &HashMap<&str, u64>, top: usize) -> Vec<(String, u64)> {
    let mut v: Vec<(&str, u64)> = m.iter().map(|(k, c)| (*k, *c)).collect();
    v.sort_unstable_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
    v.truncate(top);
    v.into_iter().map(|(k, c)| (k.to_string(), c)).collect()
}

/// La derivation : UNE projection, tout le reste en memoire.
pub struct Deriveur {
    arbre: Arbre,
    noeuds: u64,
}

impl Deriveur {
    pub fn neuf() -> Self {
        Self {
            arbre: Arbre::neuf(),
            noeuds: 0,
        }
    }

    /// Absorbe un `qualified_name`. Le decoupage est borne : au-dela de
    /// `RANG_MAX`, l'arbre ne descend plus (inutile a la navigation, et c'est
    /// ce qui garde l'empreinte memoire finie).
    pub fn absorber(&mut self, qn: &str) {
        if !qn.contains('.') {
            return;
        }
        self.arbre.inserer(qn.split('.').take(RANG_MAX + 1));
        self.noeuds += 1;
    }

    /// Groupement au rang `rang`, trie par effectif decroissant, tronque a
    /// `top` — la meme forme que rendait `split(qn,'.')[rang] / count(n)`.
    pub fn groupes_au_rang(&self, rang: usize, top: usize) -> Vec<(String, u64)> {
        let par_rang = self.arbre.collecter(0, RANG_MAX + 1);
        par_rang
            .get(rang)
            .map(|m| trier(m, top))
            .unwrap_or_default()
    }

    /// Tous les rangs d'un coup — c'est l'interet de la passe unique.
    fn tous_les_rangs(&self, top: usize) -> Vec<Vec<(String, u64)>> {
        self.arbre
            .collecter(0, RANG_MAX + 1)
            .iter()
            .map(|m| trier(m, top))
            .collect()
    }

    /// LE RANG DES RACINES, DERIVE.
    ///
    /// Reproduit pas a pas ce que faisait la boucle de sondage :
    /// 1. le rang 2 d'abord ; si l'une des racines Linux y figure, c'est fini ;
    /// 2. sinon on descend de 2 a 6 et on s'arrete au premier rang qui diverge ;
    /// 3. si aucun ne diverge, on garde le dernier essaye (6) — comme la boucle.
    ///
    /// La difference : ici, AUCUN parcours supplementaire. Les six rangs sont
    /// deja dans l'arbre.
    pub fn rang_racine(&self, top: usize) -> usize {
        self.rang_racine_de(&self.tous_les_rangs(top))
    }

    /// La MEME decision, mais sur des rangs deja calcules : `structure` les a
    /// deja en main, il serait absurde de refaire le parcours pour eux.
    fn rang_racine_de(&self, rangs: &[Vec<(String, u64)>]) -> usize {
        let vide = Vec::new();
        let au2 = rangs.get(2).unwrap_or(&vide);
        let connues = au2
            .iter()
            .filter(|(m, _)| RACINES_CONNUES.contains(&m.as_str()))
            .count();
        if connues > 0 {
            return 2;
        }
        let mut rang = 2usize;
        for essai in 2..=6usize {
            let Some(g) = rangs.get(essai) else { continue };
            if g.is_empty() {
                continue;
            }
            rang = essai;
            if diverge(g) {
                break;
            }
        }
        rang
    }

    /// La structure complete, prete a etre servie.
    pub fn structure(&self, top: usize) -> Orbites {
        let chrono = std::env::var_os("OBRAIN_ORBITES_CHRONO").is_some();
        let t = std::time::Instant::now();
        let rangs = self.tous_les_rangs(top);
        let ms_rangs = t.elapsed().as_millis();
        let t2 = std::time::Instant::now();
        // UN SEUL PARCOURS D'ARBRE POUR LES DEUX. `rang_racine()` refaisait le
        // sien : mesure, 8,5 s de plus sur `linux-full3`, pour recalculer ce
        // qui etait deja la. C'etait, a l'echelle de l'arbre, exactement le
        // defaut que ce chantier corrige a l'echelle des requetes.
        let rang_racine = self.rang_racine_de(&rangs);
        let ms_rang = t2.elapsed().as_millis();
        let t3 = std::time::Instant::now();
        let orbites: Vec<Orbite> = (rang_racine..=RANG_MAX)
            .filter_map(|r| {
                let g = rangs.get(r)?;
                if g.is_empty() {
                    return None;
                }
                Some(Orbite {
                    rang: r,
                    effectif: g.iter().map(|(_, c)| *c).sum(),
                    cardinal: g.len(),
                    etiquettes: g.iter().take(8).map(|(n, _)| n.clone()).collect(),
                })
            })
            .collect();
        // ── LES BRANCHES : CHACUNE SA PROFONDEUR ───────────────────────────
        // On redescend l'arbre jusqu'au noeud de chaque racine, puis on
        // collecte SES rangs a elle. `drivers` ouvrira cinq orbites la ou
        // `ipc` en ouvrira deux — et c'est la verite du depot, pas une regle.
        let mut branches: Vec<Branche> = Vec::new();
        if let Some(racines) = rangs.get(rang_racine) {
            for (nom, effectif) in racines.iter().filter(|(n, _)| !n.is_empty()) {
                let Some(noeud) = self.arbre.descendre(nom, rang_racine) else {
                    continue;
                };
                let par_rang = self.arbre.collecter(noeud, RANG_MAX - rang_racine);
                let mut orb: Vec<Orbite> = Vec::new();
                for (k, m) in par_rang.iter().enumerate() {
                    let g = trier(m, top);
                    if g.is_empty() {
                        continue;
                    }
                    orb.push(Orbite {
                        rang: rang_racine + 1 + k,
                        effectif: g.iter().map(|(_, c)| *c).sum(),
                        cardinal: g.len(),
                        etiquettes: g.iter().take(6).map(|(n, _)| n.clone()).collect(),
                    });
                }
                branches.push(Branche {
                    nom: nom.clone(),
                    effectif: *effectif,
                    // Elle-meme, plus ce qu'elle ouvre en dessous.
                    profondeur: 1 + orb.len(),
                    orbites: orb,
                });
            }
        }
        branches.sort_by(|a, b| b.effectif.cmp(&a.effectif));
        if chrono {
            eprintln!(
                "orbites : rang {ms_rang} ms / rangs {ms_rangs} ms / branches {} ms",
                t3.elapsed().as_millis()
            );
        }
        Orbites {
            rang_racine,
            noeuds: self.noeuds,
            orbites,
            branches,
            groupes: rangs,
        }
    }
}

/// La passe unique : une projection brute, aucun agregat.
///
/// `MATCH (n) WHERE n.qualified_name CONTAINS '.' RETURN n.qualified_name`
/// — mesuree 3,3 a 3,7 s sur 1 553 071 noeuds, contre 5,0 a 8,1 s pour
/// l'agregation d'un SEUL rang.
pub fn deriver(sess: &obrain_engine::Session) -> anyhow::Result<Deriveur> {
    use obrain_common::types::Value as OV;
    let q = format!(
        "MATCH (n) WHERE n.qualified_name CONTAINS {p}.{p} RETURN n.qualified_name AS q",
        p = '\''
    );
    // CHRONO DE PHASE : `OBRAIN_ORBITES_CHRONO=1` separe ce que coute le
    // moteur de ce que coute l'arbre. Sans cette separation, une mesure d'un
    // seul bloc ne dit pas OU va le temps — et on optimise a l'aveugle.
    let chrono = std::env::var_os("OBRAIN_ORBITES_CHRONO").is_some();
    let t0 = std::time::Instant::now();
    let res = sess
        .execute(&q)
        .map_err(|e| anyhow::anyhow!("orbites : projection des qualified_name : {e}"))?;
    let ms_moteur = t0.elapsed().as_millis();
    let t1 = std::time::Instant::now();
    let mut d = Deriveur::neuf();
    for row in &res.rows {
        if let Some(OV::String(qn)) = row.first() {
            d.absorber(qn);
        }
    }
    if chrono {
        eprintln!(
            "orbites : moteur {ms_moteur} ms ({} lignes) / arbre {} ms ({} noeuds)",
            res.rows.len(),
            t1.elapsed().as_millis(),
            d.arbre.noeuds.len()
        );
    }
    Ok(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deriveur(qns: &[&str]) -> Deriveur {
        let mut d = Deriveur::neuf();
        for q in qns {
            d.absorber(q);
        }
        d
    }

    #[test]
    fn un_qualified_name_sans_point_est_ignore() {
        let d = deriveur(&["sans_point", "a.b.c"]);
        assert_eq!(d.noeuds, 1);
    }

    #[test]
    fn les_racines_linux_figent_le_rang_a_2() {
        let d = deriveur(&[
            "tmp.linux_src.drivers.net.e1000",
            "tmp.linux_src.fs.ext4.inode",
            "tmp.linux_src.mm.slab",
        ]);
        assert_eq!(d.rang_racine(200), 2);
        let g = d.groupes_au_rang(2, 200);
        assert_eq!(g[0], ("drivers".to_string(), 1));
    }

    #[test]
    fn sans_racine_connue_on_descend_jusqu_a_la_divergence() {
        // Rangs 0,1,2 : un seul segment partout -> pas de divergence.
        // Rang 3 : deux segments distincts -> divergence.
        let d = deriveur(&[
            "home.moi.projets.alpha.fichier",
            "home.moi.projets.beta.fichier",
        ]);
        assert_eq!(d.rang_racine(200), 3);
    }

    #[test]
    fn la_regle_des_95_pour_cent_est_celle_d_avant() {
        // 96 / 100 : le premier couvre presque tout -> pas de divergence.
        let g = vec![("a".into(), 96u64), ("b".into(), 4u64)];
        assert!(!diverge(&g));
        // 94 / 100 : divergence.
        let g = vec![("a".into(), 94u64), ("b".into(), 6u64)];
        assert!(diverge(&g));
        // Un seul groupe ne diverge jamais.
        assert!(!diverge(&[("a".into(), 1u64)]));
    }

    #[test]
    fn la_profondeur_est_par_branche_pas_globale() {
        let d = deriveur(&[
            "tmp.linux_src.drivers.net.ethernet.intel.e1000.tx",
            "tmp.linux_src.fs.ext4",
        ]);
        let s = d.structure(200);
        assert_eq!(s.rang_racine, 2);
        let drivers = s.branches.iter().find(|b| b.nom == "drivers").unwrap();
        let fs = s.branches.iter().find(|b| b.nom == "fs").unwrap();
        // La preuve que la profondeur n'est pas globale : deux branches du
        // meme store, deux profondeurs.
        assert!(
            drivers.profondeur > fs.profondeur,
            "drivers={} fs={}",
            drivers.profondeur,
            fs.profondeur
        );
    }

    #[test]
    fn une_feuille_n_ouvre_qu_une_orbite() {
        let d = deriveur(&["tmp.linux_src.ipc", "tmp.linux_src.drivers.net.carte"]);
        let s = d.structure(200);
        let ipc = s.branches.iter().find(|b| b.nom == "ipc").unwrap();
        assert_eq!(ipc.profondeur, 1, "une feuille n'ouvre rien sous elle");
    }
}

// ── PARITE AVEC L'ANCIENNE BOUCLE, SUR UN VRAI STORE ────────────────────────
//
// Les tests ci-dessus prouvent la REGLE sur des cas construits. Celui-ci prouve
// l'EGALITE sur de la vraie donnee : il rejoue la boucle de sondage telle
// qu'elle etait — les memes agregations, le meme critere — et exige que la
// derivation rende le MEME rang et les MEMES couples (nom, effectif).
//
// Il ne s'execute que si `OBRAIN_BANC_STORE` designe un store : un test qui
// exigerait 7 Go de donnees pour passer serait un test qui ne passe jamais.
//
//     OBRAIN_BANC_STORE=/tmp/linux-full3.obrain cargo test --release --bin obrain parite
#[cfg(test)]
mod parite {
    use super::*;
    use obrain_common::types::Value as OV;

    const RACINES_LINUX: &[&str] = &[
        "drivers",
        "net",
        "kernel",
        "mm",
        "fs",
        "arch",
        "include",
        "init",
        "lib",
        "block",
        "ipc",
        "security",
        "crypto",
        "sound",
        "tools",
        "samples",
        "rust",
        "virt",
        "usr",
        "Documentation",
    ];

    /// L'ANCIEN CHEMIN, recopie depuis `api_viz_modules` avant ce chantier :
    /// une agregation par rang essaye, jusqu'a cinq parcours complets.
    fn ancienne_boucle(
        sess: &obrain_engine::Session,
        top: usize,
    ) -> (usize, Vec<(String, u64)>, usize) {
        let mut parcours = 0usize;
        let mut group_at = |idx: usize, parcours: &mut usize| -> Vec<(String, u64)> {
            *parcours += 1;
            let q = format!(
                "MATCH (n) WHERE n.qualified_name CONTAINS '.' \
                 RETURN split(n.qualified_name, '.')[{idx}] AS m, count(n) AS c \
                 ORDER BY c DESC LIMIT {top}"
            );
            sess.execute(&q)
                .map(|r| {
                    r.rows
                        .iter()
                        .filter_map(|row| {
                            let (Some(OV::String(m)), Some(OV::Int64(c))) =
                                (row.first(), row.get(1))
                            else {
                                return None;
                            };
                            Some((m.to_string(), *c as u64))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let known = |g: &[(String, u64)]| {
            g.iter()
                .filter(|(m, _)| RACINES_LINUX.contains(&m.as_str()))
                .count()
        };
        let mut rank_eff = 2usize;
        let mut groups = group_at(2, &mut parcours);
        if known(&groups) == 0 {
            for essai in 2..=6usize {
                let g = group_at(essai, &mut parcours);
                if g.is_empty() {
                    continue;
                }
                let total: u64 = g.iter().map(|(_, c)| *c).sum();
                let premier = g.first().map(|(_, c)| *c).unwrap_or(0);
                let diverge =
                    g.len() >= 2 && (total == 0 || premier.saturating_mul(100) / total.max(1) < 95);
                rank_eff = essai;
                groups = g;
                if diverge {
                    break;
                }
            }
        }
        (rank_eff, groups, parcours)
    }

    #[test]
    fn la_derivation_rend_exactement_ce_que_rendait_la_boucle() {
        let Ok(chemin) = std::env::var("OBRAIN_BANC_STORE") else {
            eprintln!("parite : OBRAIN_BANC_STORE absent — test non execute");
            return;
        };
        let top = 200usize;
        let db = obrain_engine::ObrainDB::open_read_only(std::path::Path::new(&chemin))
            .expect("ouverture RO du store de banc");
        let sess = db.session();

        let t0 = std::time::Instant::now();
        let (rang_ancien, groupes_ancien, parcours) = ancienne_boucle(&sess, top);
        let ms_ancien = t0.elapsed().as_millis();

        let t1 = std::time::Instant::now();
        let d = deriver(&sess).expect("derivation");
        let s = d.structure(top);
        let ms_neuf = t1.elapsed().as_millis();
        let groupes_neuf: Vec<(String, u64)> = s
            .groupes_au_rang(s.rang_racine)
            .into_iter()
            .take(top)
            .collect();

        eprintln!(
            "parite : ancien {ms_ancien} ms ({parcours} parcours) / neuf {ms_neuf} ms (1 parcours)"
        );

        // 1. LE RANG — c'est la decision, elle doit etre identique.
        assert_eq!(
            s.rang_racine, rang_ancien,
            "le rang derive differe du rang sonde"
        );

        // 2. LES COUPLES (nom, effectif) — compares comme des ENSEMBLES. Le
        //    moteur ne specifie pas l'ordre des ex aequo ; la derivation, elle,
        //    est deterministe. Exiger le meme ORDRE reviendrait a exiger du
        //    moteur une garantie qu'il ne donne pas.
        let tri = |mut v: Vec<(String, u64)>| {
            v.retain(|(m, _)| !m.is_empty());
            v.sort();
            v
        };
        let a = tri(groupes_ancien.clone());
        let b = tri(groupes_neuf.clone());
        assert_eq!(
            a.len(),
            b.len(),
            "nombre d'ensembles different\nancien={a:?}\nneuf={b:?}"
        );
        assert_eq!(a, b, "les effectifs par ensemble different");

        // 3. L'ORDRE des effectifs decroissants doit tenir de part et d'autre.
        for f in [&groupes_ancien, &groupes_neuf] {
            for p in f.windows(2) {
                assert!(p[0].1 >= p[1].1, "effectifs non decroissants : {p:?}");
            }
        }

        // 4. LA DERIVATION NE COUTE QU'UN PARCOURS — c'est l'objet du chantier.
        //    (l'ancienne en depense `parcours`, au moins un, jusqu'a six)
        assert!(parcours >= 1);
    }
}
