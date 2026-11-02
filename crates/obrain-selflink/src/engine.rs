//! Le contact avec le moteur : le self, la mémoire, les bases.
//!
//! Ce module ne connaît que des **routes HTTP du serveur viz**. Il n'ouvre
//! aucun store, ne lit aucun fichier, et n'a besoin d'aucun binaire `obrain`
//! local : c'est ce qui lui permet de tourner sur une machine qui ne porte que
//! le harness, et de parler à un moteur situé ailleurs.
//!
//! Corollaire assumé, et c'est une limite **mesurée**, pas une pudeur : l'HTTP
//! du moteur n'expose aujourd'hui que de la **lecture** (`/api/query`,
//! `/api/stores`, `/api/viz/*`, `/api/engine/*`). Les écritures durables
//! (`self/consolidate`, `import_events`, `memory_register`) ne passent que par
//! le bridge MCP en stdio. `selflink` ne peut donc pas encore consolider le
//! self — et le dire vaut mieux que de le laisser croire.

use anyhow::{Context, Result, bail};
use serde_json::Value;

use obrain_httpmin as http;

/// Adresse par défaut du serveur viz. Alignée sur le port de la prod observée.
pub const URL_DEFAUT: &str = "http://127.0.0.1:8432";

/// Le client du moteur.
pub struct Moteur {
    /// Racine, sans barre finale (ex. `http://127.0.0.1:8432`).
    pub base: String,
    /// Base visée. `None` = le défaut du serveur.
    pub store: Option<String>,
    /// Jeton IAM, si l'installation en exige un.
    pub jeton: Option<String>,
}

/// Le self, tel qu'il est **réellement** dans le graphe.
pub struct SelfNoeud {
    pub qn: String,
    /// Types d'arêtes sortantes et leur cardinalité (ex. `WEAVES` → 400).
    pub sortances: Vec<(String, i64)>,
    /// Mondes possédés : `(qn, label)`.
    pub mondes: Vec<(String, String)>,
}

impl Moteur {
    /// Construit le client depuis les arguments déjà résolus par `main`.
    pub fn nouveau(base: String, store: Option<String>, jeton: Option<String>) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
            store,
            jeton,
        }
    }

    fn url(&self, chemin: &str) -> String {
        format!("{}{}", self.base, chemin)
    }

    /// Paramètres communs : `store` n'est ajouté que s'il est connu.
    ///
    /// On ne force **jamais** un `store` par défaut : le serveur en a un, et
    /// décider à sa place ferait lire la mauvaise base sans le dire.
    fn params<'a>(&'a self, extra: &[(&'a str, &'a str)]) -> Vec<(&'a str, &'a str)> {
        let mut p: Vec<(&str, &str)> = Vec::new();
        if let Some(s) = self.store.as_deref() {
            p.push(("store", s));
        }
        p.extend_from_slice(extra);
        p
    }

    /// Signature du moteur : base par défaut, nombre de nœuds.
    pub fn info(&self) -> Result<Value> {
        http::get(&self.url("/api/info"), &[], self.jeton.as_deref())?.json()
    }

    /// Les bases (mondes) connues du serveur.
    pub fn stores(&self) -> Result<Value> {
        http::get(&self.url("/api/stores"), &[], self.jeton.as_deref())?.json()
    }

    /// Le registre des mondes tenu par le serveur (presets + ajoutés à chaud,
    /// avec leur masse mesurée).
    pub fn mondes(&self) -> Result<Value> {
        http::get(&self.url("/api/worlds"), &[], self.jeton.as_deref())?.json()
    }

    /// Résout un `--store` qui n'est **pas** un chemin absolu.
    ///
    /// # Le défaut mesuré qui justifie cette fonction
    ///
    /// Le serveur résout un store à partir du **système de fichiers**. Un nom nu
    /// — `runtime-dev`, `self-demo` — est donc lu comme un chemin **relatif au
    /// cwd du serveur**, pas comme le label d'un monde. Mesuré sur le moteur en
    /// marche :
    ///
    /// * `store=runtime-dev` → `/CHEMIN/UTILISATEUR/projects/banc-obrain/runtime-dev`,
    ///   une base vide → `count(n)` rend **0** au lieu de 424 247 ;
    /// * `store=self-demo` → rend « aucune ligne » au lieu des 2 nœuds `Self`
    ///   que porte le vrai `self-demo.obrain` ;
    /// * aucun message d'erreur, aucun avertissement.
    ///
    /// Un chiffre faux rendu silencieusement est pire qu'un refus : c'est la
    /// classe de défaut que ce dépôt combat ailleurs (« une opération qui répond
    /// « fait » sans rien faire »). On résout donc **contre les registres du
    /// serveur**, on ne devine jamais, et on refuse l'ambiguïté.
    pub fn resoudre_store(&mut self) -> Result<Option<String>> {
        let Some(demande) = self.store.clone() else {
            return Ok(None);
        };
        // Un chemin absolu est pris tel quel : c'est le seul cas non ambigu.
        if demande.starts_with('/') {
            return Ok(None);
        }

        // Candidats : les presets (`/api/stores`) et le registre des mondes
        // (`/api/worlds`). Les deux, parce que les mondes ajoutés à chaud
        // n'apparaissent pas toujours dans les presets configurés.
        let mut candidats: Vec<(String, String)> = Vec::new(); // (label, chemin)
        if let Ok(v) = self.stores() {
            if let Some(p) = v["presets"].as_array() {
                for e in p {
                    if let (Some(l), Some(c)) = (e["label"].as_str(), e["path"].as_str()) {
                        candidats.push((l.to_string(), c.to_string()));
                    }
                }
            }
        }
        if let Ok(v) = self.mondes() {
            if let Some(m) = v["mondes"].as_array() {
                for e in m {
                    if let (Some(l), Some(c)) = (e["label"].as_str(), e["path"].as_str()) {
                        candidats.push((l.to_string(), c.to_string()));
                    }
                }
            }
        }
        candidats.sort();
        candidats.dedup();

        let bas = demande.to_lowercase();
        let trouve: Vec<&(String, String)> = candidats
            .iter()
            .filter(|(label, chemin)| {
                label.to_lowercase() == bas
                    || std::path::Path::new(chemin)
                        .file_stem()
                        .map(|s| s.to_string_lossy().to_lowercase() == bas)
                        .unwrap_or(false)
                    || chemin.to_lowercase().ends_with(&format!("/{bas}"))
            })
            .collect();

        match trouve.len() {
            1 => {
                let (label, chemin) = trouve[0];
                self.store = Some(chemin.clone());
                Ok(Some(format!(
                    "base « {demande} » résolue : {label} → {chemin}"
                )))
            }
            0 => bail!(
                "base « {demande} » inconnue du moteur : ce n'est ni un chemin absolu, \
                 ni un monde déclaré ({} presets, {} mondes lus).\n\
                 Ne pas réessayer avec ce nom nu : le serveur le résoudrait RELATIF à son \
                 propre cwd et lirait une base vide SANS RIEN DIRE.\n\
                 Utiliser un chemin absolu (voir `selflink stores`) ou un label exact.",
                candidats.len(),
                candidats.len()
            ),
            n => bail!(
                "base « {demande} » ambiguë : {n} mondes correspondent —\n{}\n\
                 Trancher explicitement avec un chemin absolu.",
                trouve
                    .iter()
                    .map(|(l, c)| format!("  - {l} → {c}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ),
        }
    }

    /// Exécute une requête GQL telle quelle. Aucune interpolation : la requête
    /// est passée en paramètre, le corpus de l'utilisateur n'est pas du code.
    pub fn query(&self, gql: &str, langage: &str) -> Result<Value> {
        http::get(
            &self.url("/api/query"),
            &self.params(&[("query", gql), ("language", langage)]),
            self.jeton.as_deref(),
        )?
        .json()
    }

    /// Intentions en attente du self (`ref.self.*`).
    pub fn memoire(&self) -> Result<Value> {
        http::get(
            &self.url("/api/engine/memoire"),
            &self.params(&[]),
            self.jeton.as_deref(),
        )?
        .json()
    }

    /// Santé du moteur de conversation.
    pub fn sante(&self) -> Result<Value> {
        http::get(
            &self.url("/api/engine/sante"),
            &self.params(&[]),
            self.jeton.as_deref(),
        )?
        .json()
    }

    /// Connexion IAM : rend le jeton.
    pub fn login(&self, utilisateur: &str, mot_de_passe: &str) -> Result<Value> {
        let corps = serde_json::json!({
            "username": utilisateur,
            "password": mot_de_passe,
        })
        .to_string();
        http::post_json(&self.url("/api/auth/login"), &corps, None)?.json()
    }

    /// Lit le self : son `qn`, ses sortances par type, ses mondes.
    ///
    /// Aucun `store` n'est deviné. Sur une base sans nœud `Self` (la base
    /// `runtime-dev` par défaut, par exemple), le résultat est **vide** — et
    /// c'est une information : le self vit dans la base synaptique, pas dans
    /// celle des sessions.
    pub fn self_noeud(&self) -> Result<SelfNoeud> {
        let r = self.query("MATCH (s:Self) RETURN s.qn AS qn LIMIT 5", "gql")?;
        let qn = r["rows"]
            .as_array()
            .and_then(|lignes| lignes.first())
            .and_then(|ligne| ligne.as_array())
            .and_then(|cellules| cellules.first())
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .context(
                "aucun nœud `Self` dans cette base — le self vit dans la base synaptique \
                 (agent-synapse-*.obrain), pas dans la base des sessions ; passez --store",
            )?;

        // Les sortances : c'est la carte de ce que le self *tient*.
        let r = self.query(
            "MATCH (s:Self)-[r]->(n) RETURN type(r) AS t, count(n) AS c",
            "gql",
        )?;
        let mut sortances: Vec<(String, i64)> = Vec::new();
        if let Some(lignes) = r["rows"].as_array() {
            for ligne in lignes {
                let Some(c) = ligne.as_array() else { continue };
                let t = c.first().and_then(Value::as_str).unwrap_or("?");
                let n = c.get(1).and_then(Value::as_i64).unwrap_or(0);
                sortances.push((t.to_string(), n));
            }
        }
        sortances.sort_by(|a, b| b.1.cmp(&a.1));

        // Les mondes : ce que le self possède, nommé lisiblement.
        let r = self.query(
            "MATCH (s:Self)-[:OWNS]->(w:World) RETURN w.qn AS qn, w.label AS label LIMIT 200",
            "gql",
        )?;
        let mut mondes: Vec<(String, String)> = Vec::new();
        if let Some(lignes) = r["rows"].as_array() {
            for ligne in lignes {
                let Some(c) = ligne.as_array() else { continue };
                let q = c.first().and_then(Value::as_str).unwrap_or_default();
                let l = c.get(1).and_then(Value::as_str).unwrap_or("");
                mondes.push((
                    q.to_string(),
                    if l.is_empty() {
                        q.to_string()
                    } else {
                        l.to_string()
                    },
                ));
            }
        }
        mondes.sort();

        Ok(SelfNoeud {
            qn,
            sortances,
            mondes,
        })
    }

    /// Un tour de conversation ancré sur le self.
    ///
    /// `sur_texte` est appelé à chaque fragment : c'est ce qui permet d'afficher
    /// la réponse **pendant** qu'elle s'écrit, comme la vue plein écran du
    /// dashboard — un tour passe par le LLM local et peut prendre des secondes.
    ///
    /// Rend la raison de fin (`fin.raison`), qui dit si la réponse a été
    /// **coupée** au lieu de laisser croire à une fin naturelle.
    pub fn demander<F>(&self, question: &str, qn: Option<&str>, mut sur_texte: F) -> Result<String>
    where
        F: FnMut(&str),
    {
        let mut extra: Vec<(&str, &str)> = vec![("q", question)];
        if let Some(q) = qn {
            extra.push(("qn", q));
        }
        let params = self.params(&extra);

        let mut raison = String::new();
        let mut erreur: Option<String> = None;
        let base = self.base.clone();
        http::lire_sse(
            &self.url("/api/engine/chat"),
            &params,
            self.jeton.as_deref(),
            |nom, donnees| {
                let v: Value = serde_json::from_str(donnees).unwrap_or(Value::Null);
                match nom {
                    "texte" => {
                        if let Some(t) = v.get("t").and_then(Value::as_str) {
                            sur_texte(t);
                        }
                    }
                    "fin" => {
                        if let Some(r) = v.get("raison").and_then(Value::as_str) {
                            raison = r.to_string();
                        }
                    }
                    "erreur" => {
                        erreur = Some(
                            v.get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("erreur sans message")
                                .to_string(),
                        );
                    }
                    // `think` est ignoré par défaut : le raisonnement n'est pas
                    // la réponse, et l'afficher mêlerait les deux flux.
                    _ => {}
                }
            },
        )
        .with_context(|| format!("flux de conversation ({base}/api/engine/chat)"))?;

        if let Some(e) = erreur {
            anyhow::bail!("le moteur a signalé une erreur : {e}");
        }
        Ok(raison)
    }
}
