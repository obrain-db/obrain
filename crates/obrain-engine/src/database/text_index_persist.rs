//! ── L'INDEX TEXTE SURVIT AU PROCESSUS ───────────────────────────────────────
//!
//! Défaut mesuré le 2026-09-16 : `create_text_index` construisait l'index
//! inverti **en RAM par processus** (`substrate_text_indexes`), par un balayage
//! complet du label ; à chaque ouverture d'une base, il fallait donc le
//! reconstruire. Le prix était payé par CHAQUE lecteur (mesuré ~1,6 s sur les
//! bases runtime) — et payé pour redécouvrir ce qu'on avait déjà calculé. La
//! conséquence structurelle était pire que le coût : le BM25 était de fait
//! interdit dans un chemin chaud (une injection par tour ne peut pas se payer
//! 1,6 s de reconstruction).
//!
//! ── CE QUI EST PERSISTÉ, ET CE QUI DÉCIDE DE LA VALIDITÉ ───────────────────
//! Les PUBLICATIONS de l'index (`terme → (nœud, fréquence, longueur)`), dans un
//! fichier `substrate.textidx.<label>.<propriété>`, écrit atomiquement (tmp +
//! rename), comme les catalogues déclaratifs du substrat.
//!
//! Un index n'est valide que pour l'état du store qui l'a produit : le fichier
//! porte donc une **empreinte** (offset du WAL + nombre de nœuds vivants). Elle
//! est *conservatrice* à dessein : toute écriture invalide l'empreinte, même
//! une écriture qui ne touche pas le label indexé. Un index périmé rendrait des
//! résultats FAUX (un document ajouté serait introuvable, un document supprimé
//! resterait trouvable) — l'inverse exact du but. Le coût de cette prudence est
//! une reconstruction, c'est-à-dire le comportement d'avant ; le gain est nul
//! pour une base qui vient d'écrire, et total pour une base qu'on rouvre pour
//! lire.
//!
//! Reconstruire reste toujours possible et correct (c'est le filet) : un fichier
//! absent, illisible, d'une autre version ou d'une autre empreinte fait
//! simplement reconstruire.

use std::path::PathBuf;

#[cfg(feature = "text-index")]
use std::io::{BufReader, BufWriter, Read, Write};
#[cfg(feature = "text-index")]
use std::sync::Arc;

// L'index inverti est une capacité OPTIONNELLE du noyau (`text-index`) : sans
// elle, ce module n'a rien à persister et se réduit à des compteurs à zéro —
// ce qui est la vérité, pas un mensonge de compilation.
#[cfg(feature = "text-index")]
use obrain_core::index::text::{BM25Config, InvertedIndex, SimpleTokenizer};

/// Signature du fichier — un format qui se reconnaît, et qui se refuse.
const MAGIC: &[u8; 8] = b"OBTIDX01";

impl super::ObrainDB {
    /// Le chemin du fichier d'index texte d'un couple (label, propriété).
    ///
    /// Le nom est échappé : un label ou une propriété peut porter un caractère
    /// interdit par le système de fichiers, et un nom mal échappé écrirait
    /// ailleurs — on encode donc tout ce qui n'est pas alphanumérique en `%XX`.
    #[cfg(feature = "text-index")]
    fn chemin_index_texte(&self, label: &str, property: &str) -> Option<PathBuf> {
        let base = self.config.path.as_ref()?;
        Some(base.join(format!(
            "substrate.textidx.{}.{}",
            echapper_nom(label),
            echapper_nom(property)
        )))
    }

    /// L'empreinte de l'état du store : `(offset du WAL, nœuds vivants)`.
    ///
    /// Elle change à la moindre écriture — volontairement. Un index texte ne
    /// sait pas quelles écritures le concernent ; prétendre le savoir
    /// demanderait de tenir un journal par index, et se tromper rendrait des
    /// résultats faux. Voir l'en-tête de ce module.
    #[cfg(feature = "text-index")]
    fn empreinte_store(&self) -> (u64, u64) {
        let offset = self
            .substrate_store
            .as_ref()
            .map_or(0, |s| s.writer().wal().offset());
        (offset, self.data_store().node_ids().len() as u64)
    }

    /// Persiste l'index d'un couple (label, propriété). Best-effort : un échec
    /// est journalisé, jamais fatal — l'index en RAM sert la session courante,
    /// la reconstruction reste le filet des suivantes.
    #[cfg(feature = "text-index")]
    pub(super) fn persister_index_texte(&self, label: &str, property: &str, index: &InvertedIndex) {
        let Some(chemin) = self.chemin_index_texte(label, property) else {
            return; // base en mémoire : rien à persister, et c'est honnête
        };
        let (offset, noeuds) = self.empreinte_store();
        let temporaire = chemin.with_extension("tmp");
        let ecrire = || -> std::io::Result<()> {
            let fichier = std::fs::File::create(&temporaire)?;
            let mut w = BufWriter::new(fichier);
            w.write_all(MAGIC)?;
            w.write_all(&offset.to_le_bytes())?;
            w.write_all(&noeuds.to_le_bytes())?;
            // ── QUI A PRODUIT CET INDEX ? ────────────────────────────────────
            // Les termes stockés SONT la sortie d'un tokenizer : recharger ce
            // fichier avec un autre tokenizer (mots vides, longueur minimale,
            // stemmer, langue) rendrait un vocabulaire périmé SANS ERREUR. On
            // l'écrit donc, et on refuse au chargement ce qui ne correspond plus.
            let identite = index.identite();
            w.write_all(&(identite.len() as u32).to_le_bytes())?;
            w.write_all(identite.as_bytes())?;
            let postings = index.exporter_postings();
            w.write_all(&(postings.len() as u64).to_le_bytes())?;
            for (terme, pubs) in postings {
                w.write_all(&(terme.len() as u32).to_le_bytes())?;
                w.write_all(terme.as_bytes())?;
                w.write_all(&(pubs.len() as u32).to_le_bytes())?;
                for (id, tf, doc_len) in pubs {
                    w.write_all(&id.to_le_bytes())?;
                    w.write_all(&tf.to_le_bytes())?;
                    w.write_all(&doc_len.to_le_bytes())?;
                }
            }
            w.flush()?;
            w.get_ref().sync_all()?;
            Ok(())
        };
        if let Err(e) = ecrire().and_then(|()| std::fs::rename(&temporaire, &chemin)) {
            tracing::warn!(
                erreur = %e, label, property,
                "index texte NON persiste — il sera reconstruit a la prochaine ouverture"
            );
            return;
        }
        // L'empreinte du moment est retenue : c'est elle qui dira, à la
        // fermeture, si cet index a vu une écriture depuis sa construction.
        self.index_texte_empreintes
            .write()
            .insert(format!("{label}:{property}"), offset);
        self.index_texte_epochs.write().insert(
            format!("{label}:{property}"),
            self.data_store().current_epoch().as_u64(),
        );
        self.index_texte_persistes
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }

    /// Charge l'index d'un couple (label, propriété) si le fichier existe ET que
    /// son empreinte correspond à l'état COURANT du store.
    ///
    /// Rend `None` — « reconstruis » — dans tous les autres cas : fichier
    /// absent, illisible, signature inconnue, empreinte périmée. Aucun de ces
    /// cas n'est une erreur : ce sont les raisons d'être du filet.
    #[cfg(feature = "text-index")]
    pub(super) fn charger_index_texte(
        &self,
        label: &str,
        property: &str,
    ) -> Option<Arc<parking_lot::RwLock<InvertedIndex>>> {
        let chemin = self.chemin_index_texte(label, property)?;
        let fichier = std::fs::File::open(&chemin).ok()?;
        let mut r = BufReader::new(fichier);
        let mut magic = [0u8; 8];
        let mut u64buf = [0u8; 8];
        let mut u32buf = [0u8; 4];

        r.read_exact(&mut magic).ok()?;
        if &magic != MAGIC {
            return None;
        }
        r.read_exact(&mut u64buf).ok()?;
        let offset_fichier = u64::from_le_bytes(u64buf);
        r.read_exact(&mut u64buf).ok()?;
        let noeuds_fichier = u64::from_le_bytes(u64buf);
        let (offset, noeuds) = self.empreinte_store();
        if (offset_fichier, noeuds_fichier) != (offset, noeuds) {
            // EMPREINTE PÉRIMÉE : l'index ne décrit plus ce store. On ne le
            // charge pas — un index périmé rend des résultats faux, et c'est
            // exactement ce qu'on refuse.
            return None;
        }
        r.read_exact(&mut u32buf).ok()?;
        let len_identite = u32::from_le_bytes(u32buf) as usize;
        let mut identite_brute = vec![0u8; len_identite];
        r.read_exact(&mut identite_brute).ok()?;
        let identite_fichier = String::from_utf8(identite_brute).ok()?;
        // L'identité attendue est celle que produirait le code ACTUEL : on la
        // demande à un index vide plutôt que de la recomposer ici (une seule
        // source de vérité, donc pas de dérive entre les deux).
        let identite_attendue = InvertedIndex::new(BM25Config::default()).identite();
        if identite_fichier != identite_attendue {
            // REFUS EXPLICITE : le fichier a été produit par un autre vocabulaire
            // (ou une autre config BM25). Le charger rendrait des réponses que le
            // code ne saurait plus expliquer.
            tracing::info!(
                fichier = %identite_fichier, attendu = %identite_attendue,
                "index texte IGNORE : produit par un autre tokenizer/config — reconstruction"
            );
            return None;
        }
        r.read_exact(&mut u64buf).ok()?;
        let n_termes = u64::from_le_bytes(u64buf) as usize;
        let mut postings: Vec<(String, Vec<(u64, u32, u32)>)> = Vec::with_capacity(n_termes);
        for _ in 0..n_termes {
            r.read_exact(&mut u32buf).ok()?;
            let len_terme = u32::from_le_bytes(u32buf) as usize;
            let mut brut = vec![0u8; len_terme];
            r.read_exact(&mut brut).ok()?;
            let terme = String::from_utf8(brut).ok()?;
            r.read_exact(&mut u32buf).ok()?;
            let n_pubs = u32::from_le_bytes(u32buf) as usize;
            let mut pubs = Vec::with_capacity(n_pubs);
            for _ in 0..n_pubs {
                let mut id = [0u8; 8];
                let mut tf = [0u8; 4];
                let mut dl = [0u8; 4];
                r.read_exact(&mut id).ok()?;
                r.read_exact(&mut tf).ok()?;
                r.read_exact(&mut dl).ok()?;
                pubs.push((
                    u64::from_le_bytes(id),
                    u32::from_le_bytes(tf),
                    u32::from_le_bytes(dl),
                ));
            }
            postings.push((terme, pubs));
        }
        let index = InvertedIndex::depuis_postings(
            BM25Config::default(),
            Box::new(SimpleTokenizer::new()),
            postings,
        );
        // Un index chargé est À JOUR par construction (c'est la condition du
        // chargement) : son empreinte est celle du store tel qu'il est.
        self.index_texte_empreintes
            .write()
            .insert(format!("{label}:{property}"), offset);
        self.index_texte_epochs.write().insert(
            format!("{label}:{property}"),
            self.data_store().current_epoch().as_u64(),
        );
        self.index_texte_charges
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Some(Arc::new(parking_lot::RwLock::new(index)))
    }

    /// Repersiste les index texte **À JOUR**, et efface les PÉRIMÉS.
    ///
    /// ── POURQUOI CETTE DISTINCTION, MESURÉE ─────────────────────────────────
    /// L'empreinte retenue est l'offset du WAL, parce que c'est le seul signal
    /// qui bouge sur TOUT ce qui compte : une création, une suppression, et
    /// aussi une simple écriture de propriété (mesuré : 551 → 675 sur un `SET`,
    /// alors que les compteurs de nœuds et d'arêtes, eux, ne bougent pas — un
    /// index validé par les seuls compteurs chargerait un texte périmé).
    ///
    /// Mais l'offset bouge AUSSI pendant la fermeture elle-même (mesuré :
    /// 551 → 583, les écritures de flush) : un index persisté à sa construction
    /// porte donc une empreinte qui ne correspond plus à la réouverture. D'où ce
    /// passage à la fermeture, APRÈS le flush — à l'état exact où la base
    /// s'arrête :
    ///
    /// * empreinte de construction == offset courant ⇒ l'index n'a rien vu
    ///   passer depuis : on le réécrit avec l'empreinte de fermeture, et la
    ///   réouverture le chargera (**c'est le gain**) ;
    /// * empreinte différente ⇒ il est PÉRIMÉ : on SUPPRIME son fichier, pour
    ///   qu'aucune réouverture ne puisse le croire. Un index périmé rend des
    ///   résultats faux — un document ajouté introuvable, un texte modifié
    ///   toujours cherché à l'ancienne.
    #[cfg(feature = "text-index")]
    pub(super) fn repersister_index_textuels(&self) {
        if self.substrate_store.is_none() {
            return; // base en mémoire : rien de durable à écrire
        }
        let (offset, _) = self.empreinte_store();
        let cles: Vec<String> = self.substrate_text_indexes.read().keys().cloned().collect();
        for cle in cles {
            let Some((label, property)) = cle.split_once(':') else {
                continue;
            };
            // C'est l'ÉPOQUE qui décide ici : elle ne bouge pas à la fermeture,
            // et elle bouge à chaque écriture (voir `index_texte_epochs`).
            let a_jour = self
                .index_texte_epochs
                .read()
                .get(&cle)
                .copied()
                .is_some_and(|e| e == self.data_store().current_epoch().as_u64());
            if a_jour {
                let garde = self.substrate_text_indexes.read();
                if let Some(index) = garde.get(&cle) {
                    let index = index.read();
                    self.persister_index_texte(label, property, &index);
                }
            } else {
                // PÉRIMÉ : le fichier ne doit pas survivre à la fermeture.
                if let Some(chemin) = self.chemin_index_texte(label, property) {
                    let _ = std::fs::remove_file(chemin);
                }
                self.index_texte_empreintes.write().remove(&cle);
                self.index_texte_epochs.write().remove(&cle);
            }
        }
    }

    /// Le nombre d'index texte CHARGÉS depuis le disque (introspection).
    #[must_use]
    pub fn nb_index_texte_charges(&self) -> usize {
        self.index_texte_charges
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Le nombre d'index texte PERSISTÉS sur le disque (introspection).
    #[must_use]
    pub fn nb_index_texte_persistes(&self) -> usize {
        self.index_texte_persistes
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Hydrate les index texte DÉCLARÉS au catalogue du substrat.
    ///
    /// Appelé à l'ouverture d'une base substrate, à côté de l'hydratation des
    /// procédures et des types : une base rouverte pour LIRE retrouve ses index
    /// sans les reconstruire. Un index dont l'empreinte ne correspond plus est
    /// reconstruit par le chemin normal (`create_text_index`), qui le
    /// repersiste — le comportement d'avant, en repli.
    #[cfg(feature = "text-index")]
    pub(super) fn hydrater_index_textuels(&self) {
        let Some(ref sub) = self.substrate_store else {
            return;
        };
        let declarations = sub.declarations_index();
        let mut charges = 0usize;
        let mut differes = 0usize;
        for decl in declarations {
            if decl.genre != "text" {
                continue;
            }
            let (label, property) = (decl.label.as_str(), decl.propriete.as_str());
            let key = format!("{label}:{property}");
            if let Some(index) = self.charger_index_texte(label, property) {
                self.substrate_text_indexes.write().insert(key, index);
                charges += 1;
                continue;
            }
            // ── ON DIFFÈRE, ON NE RECONSTRUIT PAS (défaut mesuré) ───────────
            // Première version de cette hydratation : elle RECONSTRUISAIT tout
            // index déclaré dont le fichier manquait ou ne correspondait plus.
            // Défaut mesuré le 2026-09-17 sur le store réel (484 000 nœuds, 12
            // index texte déclarés), par les phases déjà instrumentées de
            // l'ouverture :
            //
            // ```text
            // index texte hydrates … charges=0 reconstruits=12
            // obrain-mcp: store ouvert (…, RW, 5930 ms)
            // ```
            //
            // **5,9 s des 7,8 s de la vie du pont** — et cela À CHAQUE
            // OUVERTURE, alors que le harness **respawn un pont par requête**.
            // Une « optimisation » qui reconstruit douze balayages complets de
            // label à chaque démarrage n'optimise rien : elle transforme un coût
            // PARESSEUX (l'index se construisait à la première recherche qui en
            // avait besoin) en coût SYSTÉMATIQUE.
            //
            // On revient donc au comportement paresseux : on n'installe ici que
            // ce qu'on a pu LIRE du disque — ce qui est le gain visé — et on
            // laisse la reconstruction à la première recherche qui la demande
            // (`create_text_index`, inchangé). Le fichier persisté reste écrit
            // après chaque construction, donc l'ouverture SUIVANTE chargera.
            differes += 1;
        }
        if charges > 0 || differes > 0 {
            tracing::info!(
                charges,
                differes,
                "index texte : {} chargés depuis substrate.textidx.*, {} différés \
                 (reconstruction à la première recherche, par le chemin paresseux)",
                charges,
                differes
            );
        }
    }
}

/// Échappe un nom pour qu'il soit utilisable comme composant de fichier.
///
/// Tout ce qui n'est pas `[A-Za-z0-9_-]` devient `%XX` : un label contenant
/// `/`, `..` ou un espace ne peut donc pas écrire ailleurs que dans le
/// répertoire du store, ni produire un nom ambigu.
#[cfg(feature = "text-index")]
fn echapper_nom(nom: &str) -> String {
    let mut sortie = String::with_capacity(nom.len());
    for octet in nom.bytes() {
        match octet {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' => sortie.push(octet as char),
            _ => sortie.push_str(&format!("%{octet:02X}")),
        }
    }
    sortie
}
