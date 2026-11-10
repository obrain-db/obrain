//! Projection apprise d'un espace de plongements à dimension native (768, 1 024…)
//! vers la largeur fixe du palier L2 du cascade ([`L2_DIM`] = 384).
//!
//! ## Pourquoi (décision 6f9602af, 10/11/2026)
//!
//! Le cascade par paliers (`tiered_scan`) ne sait comparer que des vecteurs de
//! [`L2_DIM`] composantes : `Tier2 = [u16; L2_DIM]` est un format de zone. Un
//! plongement plus large doit donc être **projeté**, et la façon de le faire
//! décide du rappel. MESURÉ sur 32 521 plongements réels 768-d : une
//! **troncature** aux 384 premières coordonnées rend recall@10 = 0,606 ; la
//! **projection PCA** apprise sur les données rend 0,846 sans rerank, 0,986
//! avec rerank exact (k0 = 4 000). La PCA fait aussi mieux que des projections
//! aléatoires calculées sur les 768 natifs : elle décorrèle, et le hachage par
//! signes des paliers L0/L1 s'en nourrit.
//!
//! ## Ce que ce module garantit
//!
//! * **Déterminisme** : même échantillon, même graine ⇒ même matrice, au bit
//!   près. Un index persisté se recharge donc avec exactement la projection
//!   qui l'a construit.
//! * **Orthonormalité** des composantes (Gram-Schmidt modifié à chaque
//!   itération) : la projection préserve les angles au mieux de ce que 384
//!   dimensions permettent.
//! * **Un seul apprentissage** : la projection est ajustée à la création de
//!   l'index et jamais ré-ajustée implicitement — les paliers n'ont de sens
//!   qu'avec elle. La ré-ajuster, c'est reconstruire l'index (`rebuild`).
//! * **Persistance atomique** : fichier `substrate.vecproj.<label>.<prop>`
//!   écrit par `tmp` + `rename` + `fsync`, en-tête versionné et CRC32 du
//!   payload. Un fichier absent se lit `Ok(None)` ; un fichier corrompu ou
//!   d'une version inconnue se lit `Err` — l'appelant reconstruit, il ne
//!   devine jamais.
//! * Le fichier est une zone nommée au sens de `SubstrateFile::zone_names` :
//!   l'instantané navigateur l'emporte sans travail supplémentaire.
//!
//! ## Ce que ce module ne fait pas
//!
//! Il ne choisit pas l'échantillon (l'appelant le prélève dans la colonne de
//! vecteurs) et ne touche ni aux paliers ni au catalogue d'index : c'est la
//! tâche C4 du plan O-VEC. Pour une dimension d'entrée ≤ [`L2_DIM`], la
//! projection est l'**identité** complétée de zéros — le comportement
//! antérieur (`rembourrer_au_cascade`), conservé tel quel.

use crate::error::{SubstrateError, SubstrateResult};
use crate::tiers::L2_DIM;
use rayon::prelude::*;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// Signature des fichiers de projection, lisible dans un `hexdump`.
pub const MAGIC: &[u8; 8] = b"OBVPRJ01";
/// Version du format. À incrémenter si l'en-tête, le payload ou la méthode
/// d'ajustement (qui conditionne la reproductibilité) changent.
pub const VERSION: u16 = 1;
/// Itérations de sous-espace. MESURÉ : 10 suffisent pour 96,7 % de variance
/// expliquée sur les plongements réels (échantillon 20 000).
pub const ITERATIONS: usize = 10;
/// Taille d'échantillon au-delà de laquelle l'ajustement n'apprend plus rien
/// d'utile et coûte du temps : l'appelant sous-échantillonne à ce plafond.
pub const ECHANTILLON_MAX: usize = 20_000;

const FLAG_IDENTITE: u16 = 1 << 0;
const PREFIXE_FICHIER: &str = "substrate.vecproj.";
/// Taille de l'en-tête sérialisé : `magic[8] version:u16 flags:u16 dim_entree:u32
/// dim_sortie:u32 n_echantillon:u32 graine:u64 trace:f32 crc32:u32`.
const ENTETE: usize = 8 + 2 + 2 + 4 + 4 + 4 + 8 + 4 + 4;

/// Une projection vers [`L2_DIM`] composantes : identité (dimension d'entrée
/// ≤ 384) ou PCA apprise (dimension d'entrée > 384).
#[derive(Clone, Debug, PartialEq)]
pub enum Projection {
    /// Copie des coordonnées, complétée de zéros jusqu'à [`L2_DIM`].
    Identite {
        /// Dimension des vecteurs d'entrée (1 ..= [`L2_DIM`]).
        dim_entree: usize,
    },
    /// Projection PCA apprise sur un échantillon.
    Apprise(ProjectionApprise),
}

/// Projection PCA : centrage puis produit par une matrice orthonormée
/// `dim_sortie × dim_entree` (ligne-major), puis renormalisation.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectionApprise {
    dim_entree: usize,
    dim_sortie: usize,
    moyenne: Vec<f32>,
    /// `dim_sortie` lignes de `dim_entree` colonnes : `matrice[k * dim_entree + d]`.
    matrice: Vec<f32>,
    /// Valeurs propres estimées des composantes retenues (ordre d'itération).
    lambda: Vec<f32>,
    /// Trace de la covariance de l'échantillon (variance totale).
    trace: f32,
    n_echantillon: u32,
    graine: u64,
}

impl Projection {
    /// Projection identité pour une dimension d'entrée ≤ [`L2_DIM`].
    pub fn identite(dim_entree: usize) -> SubstrateResult<Self> {
        if dim_entree == 0 || dim_entree > L2_DIM {
            return Err(SubstrateError::Internal(format!(
                "projection identité : dim_entree {dim_entree} hors de 1..={L2_DIM}"
            )));
        }
        Ok(Projection::Identite { dim_entree })
    }

    /// Dimension des vecteurs que cette projection accepte en entrée.
    pub fn dim_entree(&self) -> usize {
        match self {
            Projection::Identite { dim_entree } => *dim_entree,
            Projection::Apprise(p) => p.dim_entree,
        }
    }

    /// Projette `v` (longueur [`Self::dim_entree`]) en [`L2_DIM`] composantes,
    /// renormalisé. Erreur si la longueur ne correspond pas : on ne devine pas.
    pub fn projeter(&self, v: &[f32]) -> SubstrateResult<Vec<f32>> {
        if v.len() != self.dim_entree() {
            return Err(SubstrateError::Internal(format!(
                "projeter : vecteur de {} composantes, projection attendue sur {}",
                v.len(),
                self.dim_entree()
            )));
        }
        Ok(match self {
            Projection::Identite { .. } => {
                let mut out = vec![0f32; L2_DIM];
                out[..v.len()].copy_from_slice(v);
                out
            }
            Projection::Apprise(p) => p.projeter(v),
        })
    }

    /// Nom de fichier de la zone pour un index `(label, propriété)`.
    pub fn nom_fichier(label: &str, propriete: &str) -> String {
        format!(
            "{PREFIXE_FICHIER}{}.{}",
            echapper(label),
            echapper(propriete)
        )
    }

    /// Écrit la projection dans `dir` sous [`Self::nom_fichier`], atomiquement
    /// (fichier temporaire, `fsync`, `rename`).
    pub fn ecrire(&self, dir: &Path, label: &str, propriete: &str) -> SubstrateResult<()> {
        let chemin = dir.join(Self::nom_fichier(label, propriete));
        let temporaire = chemin.with_extension("vecproj.tmp");
        let octets = self.en_octets();
        {
            let mut f = std::fs::File::create(&temporaire)?;
            f.write_all(&octets)?;
            f.sync_all()?;
        }
        std::fs::rename(&temporaire, &chemin)?;
        Ok(())
    }

    /// Écrit la projection dans la ZONE nommée [`Self::nom_fichier`] de `sub`
    /// (mode fichier ou mémoire : même chemin que les paliers d'un index
    /// nommé, donc emportée par l'instantané et visible de `zone_names`).
    /// La zone est écrite en place ; le CRC du payload protège une écriture
    /// interrompue (relue ⇒ `Err` ⇒ reconstruction).
    pub fn ecrire_zone(
        &self,
        sub: &crate::file::SubstrateFile,
        label: &str,
        propriete: &str,
    ) -> SubstrateResult<()> {
        let octets = self.en_octets();
        let mut zf = sub.open_named_zone(&Self::nom_fichier(label, propriete))?;
        crate::writer::ensure_room(&mut zf, octets.len(), 0)?;
        zf.as_slice_mut()[..octets.len()].copy_from_slice(&octets);
        zf.msync()?;
        zf.fsync()?;
        Ok(())
    }

    /// Relit la projection depuis sa zone nommée. `Ok(None)` si la zone est
    /// absente ou vide ; `Err` si son contenu est invalide (signature,
    /// version, taille, CRC). La zone peut être plus longue que le contenu
    /// (`ensure_room` arrondit) : la longueur utile vient de l'en-tête.
    pub fn lire_zone(
        sub: &crate::file::SubstrateFile,
        label: &str,
        propriete: &str,
    ) -> SubstrateResult<Option<Self>> {
        let Ok(zf) = sub.open_named_zone_read_only(&Self::nom_fichier(label, propriete)) else {
            return Ok(None);
        };
        let slice = zf.as_slice();
        if slice.len() < ENTETE || slice[..8].iter().all(|b| *b == 0) {
            return Ok(None);
        }
        let total = Self::taille_totale_depuis_entete(slice)?;
        if slice.len() < total {
            return Err(SubstrateError::Internal(format!(
                "projection : zone de {} octets, {total} attendus d'après l'en-tête",
                slice.len()
            )));
        }
        Self::depuis_octets(&slice[..total]).map(Some)
    }

    /// Longueur totale (en-tête + payload) annoncée par un en-tête valide.
    fn taille_totale_depuis_entete(octets: &[u8]) -> SubstrateResult<usize> {
        if octets.len() < ENTETE {
            return Err(SubstrateError::Internal(format!(
                "projection : {} octets, en-tête de {ENTETE} attendu",
                octets.len()
            )));
        }
        if &octets[..8] != MAGIC {
            return Err(SubstrateError::BadMagic);
        }
        let flags = u16::from_le_bytes([octets[10], octets[11]]);
        if flags & FLAG_IDENTITE != 0 {
            return Ok(ENTETE);
        }
        let d = u32::from_le_bytes([octets[12], octets[13], octets[14], octets[15]]) as usize;
        let k = u32::from_le_bytes([octets[16], octets[17], octets[18], octets[19]]) as usize;
        Ok(ENTETE + 4 * (d + k + k * d))
    }

    /// Relit la projection de `dir`. `Ok(None)` si le fichier n'existe pas ;
    /// `Err` si la signature, la version, la taille ou le CRC ne correspondent
    /// pas — l'appelant reconstruit l'index, il n'interprète pas.
    pub fn lire(dir: &Path, label: &str, propriete: &str) -> SubstrateResult<Option<Self>> {
        let chemin = dir.join(Self::nom_fichier(label, propriete));
        let mut f = match std::fs::File::open(&chemin) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e.into()),
        };
        let mut octets = Vec::new();
        f.read_to_end(&mut octets)?;
        Self::depuis_octets(&octets).map(Some)
    }

    /// Sérialisation complète (en-tête + payload). Format v1, petit-boutiste :
    /// `magic[8] version:u16 flags:u16 dim_entree:u32 dim_sortie:u32
    ///  n_echantillon:u32 graine:u64 trace:f32 crc32:u32` puis, hors identité,
    /// `moyenne f32[dim_entree]`, `lambda f32[dim_sortie]`,
    /// `matrice f32[dim_sortie * dim_entree]`.
    pub fn en_octets(&self) -> Vec<u8> {
        let (flags, dim_entree, dim_sortie, n_ech, graine, trace, payload) = match self {
            Projection::Identite { dim_entree } => (
                FLAG_IDENTITE,
                *dim_entree as u32,
                L2_DIM as u32,
                0u32,
                0u64,
                0f32,
                Vec::new(),
            ),
            Projection::Apprise(p) => {
                let mut payload =
                    Vec::with_capacity(4 * (p.moyenne.len() + p.lambda.len() + p.matrice.len()));
                for x in p.moyenne.iter().chain(&p.lambda).chain(&p.matrice) {
                    payload.extend_from_slice(&x.to_le_bytes());
                }
                (
                    0u16,
                    p.dim_entree as u32,
                    p.dim_sortie as u32,
                    p.n_echantillon,
                    p.graine,
                    p.trace,
                    payload,
                )
            }
        };
        let mut h = crc32fast::Hasher::new();
        h.update(&payload);
        let crc = h.finalize();
        let mut out = Vec::with_capacity(40 + payload.len());
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&VERSION.to_le_bytes());
        out.extend_from_slice(&flags.to_le_bytes());
        out.extend_from_slice(&dim_entree.to_le_bytes());
        out.extend_from_slice(&dim_sortie.to_le_bytes());
        out.extend_from_slice(&n_ech.to_le_bytes());
        out.extend_from_slice(&graine.to_le_bytes());
        out.extend_from_slice(&trace.to_le_bytes());
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&payload);
        out
    }

    /// Désérialisation stricte : toute incohérence est une erreur.
    pub fn depuis_octets(octets: &[u8]) -> SubstrateResult<Self> {
        if octets.len() < ENTETE {
            return Err(SubstrateError::Internal(format!(
                "projection : {} octets, en-tête de {ENTETE} attendu",
                octets.len()
            )));
        }
        if &octets[..8] != MAGIC {
            return Err(SubstrateError::BadMagic);
        }
        let mut pos = 8;
        let mut u16_ = |o: &[u8]| {
            let v = u16::from_le_bytes([o[pos], o[pos + 1]]);
            pos += 2;
            v
        };
        let version = u16_(octets);
        let flags = u16_(octets);
        let lire_u32 = |pos: &mut usize| {
            let v = u32::from_le_bytes([
                octets[*pos],
                octets[*pos + 1],
                octets[*pos + 2],
                octets[*pos + 3],
            ]);
            *pos += 4;
            v
        };
        if version != VERSION {
            return Err(SubstrateError::UnsupportedVersion(
                u32::from(version),
                u32::from(VERSION),
            ));
        }
        let dim_entree = lire_u32(&mut pos) as usize;
        let dim_sortie = lire_u32(&mut pos) as usize;
        let n_echantillon = lire_u32(&mut pos);
        let graine = u64::from_le_bytes(octets[pos..pos + 8].try_into().expect("8 octets"));
        pos += 8;
        let trace = f32::from_le_bytes(octets[pos..pos + 4].try_into().expect("4 octets"));
        pos += 4;
        let crc_attendu = lire_u32(&mut pos);
        let payload = &octets[pos..];
        let mut h = crc32fast::Hasher::new();
        h.update(payload);
        let crc_calcule = h.finalize();
        if crc_calcule != crc_attendu {
            return Err(SubstrateError::Internal(format!(
                "projection : CRC32 du payload invalide (annoncé {crc_attendu:#010x}, calculé {crc_calcule:#010x})"
            )));
        }
        if dim_sortie != L2_DIM {
            return Err(SubstrateError::Internal(format!(
                "projection : dim_sortie {dim_sortie} ≠ L2_DIM {L2_DIM}"
            )));
        }
        if flags & FLAG_IDENTITE != 0 {
            if !payload.is_empty() {
                return Err(SubstrateError::Internal(
                    "projection identité avec payload".into(),
                ));
            }
            return Self::identite(dim_entree);
        }
        let attendu = 4 * (dim_entree + dim_sortie + dim_sortie * dim_entree);
        if payload.len() != attendu {
            return Err(SubstrateError::Internal(format!(
                "projection : payload de {} octets, {attendu} attendus",
                payload.len()
            )));
        }
        let mut f32s = payload
            .as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c));
        let moyenne: Vec<f32> = f32s.by_ref().take(dim_entree).collect();
        let lambda: Vec<f32> = f32s.by_ref().take(dim_sortie).collect();
        let matrice: Vec<f32> = f32s.collect();
        Ok(Projection::Apprise(ProjectionApprise {
            dim_entree,
            dim_sortie,
            moyenne,
            matrice,
            lambda,
            trace,
            n_echantillon,
            graine,
        }))
    }
}

impl ProjectionApprise {
    /// Ajuste une PCA `dim_entree → L2_DIM` sur `echantillon` (vecteurs
    /// concaténés, `echantillon.len()` multiple de `dim_entree`), par
    /// itération de sous-espace ([`ITERATIONS`] passes), initialisation
    /// pseudo-aléatoire déterministe par `graine`.
    ///
    /// Exigences : `dim_entree > L2_DIM` (sinon [`Projection::identite`]),
    /// au moins deux vecteurs. Au-delà de [`ECHANTILLON_MAX`] vecteurs,
    /// seuls les premiers [`ECHANTILLON_MAX`] sont utilisés (l'appelant
    /// choisit lesquels).
    pub fn ajuster(echantillon: &[f32], dim_entree: usize, graine: u64) -> SubstrateResult<Self> {
        if dim_entree <= L2_DIM {
            return Err(SubstrateError::Internal(format!(
                "ajuster : dim_entree {dim_entree} ≤ L2_DIM {L2_DIM}, utiliser Projection::identite"
            )));
        }
        if !echantillon.len().is_multiple_of(dim_entree) {
            return Err(SubstrateError::Internal(format!(
                "ajuster : {} f32 n'est pas un multiple de dim_entree {dim_entree}",
                echantillon.len()
            )));
        }
        let n = (echantillon.len() / dim_entree).min(ECHANTILLON_MAX);
        if n < 2 {
            return Err(SubstrateError::Internal(format!(
                "ajuster : {n} vecteur(s), au moins 2 requis"
            )));
        }
        let d = dim_entree;
        let k = L2_DIM;
        let lignes: Vec<&[f32]> = (0..n).map(|i| &echantillon[i * d..(i + 1) * d]).collect();

        // Moyenne.
        let mut moyenne = vec![0f32; d];
        for v in &lignes {
            for (m, x) in moyenne.iter_mut().zip(*v) {
                *m += x;
            }
        }
        for m in moyenne.iter_mut() {
            *m /= n as f32;
        }

        // Échantillon centré, n × d, ligne-major.
        let centre: Vec<f32> = lignes
            .iter()
            .flat_map(|v| v.iter().zip(&moyenne).map(|(x, m)| x - m))
            .collect();

        // Covariance d × d. Le parallélisme porte sur la LIGNE i (pas sur
        // l'échantillon) : chaque coefficient est une somme sur les n vecteurs
        // dans un ordre fixe, donc le résultat est identique au bit près d'une
        // exécution à l'autre quel que soit le découpage de rayon. (Un
        // fold/reduce sur l'échantillon additionne dans un ordre qui dépend du
        // découpage et cassait `ajuster_est_deterministe`.) Seul le triangle
        // supérieur est calculé, puis recopié par symétrie.
        let inv_n = 1.0 / n as f32;
        let triangle: Vec<Vec<f32>> = (0..d)
            .into_par_iter()
            .map(|i| {
                let mut ligne = vec![0f32; d - i];
                for v in centre.chunks_exact(d) {
                    let ci = v[i];
                    if ci == 0.0 {
                        continue;
                    }
                    for (a, cj) in ligne.iter_mut().zip(&v[i..]) {
                        *a += ci * cj;
                    }
                }
                for a in ligne.iter_mut() {
                    *a *= inv_n;
                }
                ligne
            })
            .collect();
        let mut cov = vec![0f32; d * d];
        for (i, ligne) in triangle.iter().enumerate() {
            for (dj, &x) in ligne.iter().enumerate() {
                let j = i + dj;
                cov[i * d + j] = x;
                cov[j * d + i] = x;
            }
        }
        drop(centre);
        let trace: f32 = (0..d).map(|i| cov[i * d + i]).sum();

        // Q : d × k, colonnes orthonormées, initialisation déterministe.
        let mut rng = SplitMix(graine);
        let mut q = vec![0f32; d * k]; // q[i * k + j]
        for x in q.iter_mut() {
            *x = rng.gauss();
        }
        gram_schmidt(&mut q, d, k);
        let mut lambda = vec![0f32; k];
        for _ in 0..ITERATIONS {
            let qr: &[f32] = &q;
            let z: Vec<f32> = (0..d)
                .into_par_iter()
                .flat_map_iter(|i| {
                    let ligne = &cov[i * d..(i + 1) * d];
                    (0..k).map(move |j| (0..d).map(|l| ligne[l] * qr[l * k + j]).sum::<f32>())
                })
                .collect();
            for (j, lam) in lambda.iter_mut().enumerate() {
                *lam = (0..d)
                    .map(|i| z[i * k + j] * z[i * k + j])
                    .sum::<f32>()
                    .sqrt();
            }
            q = z;
            gram_schmidt(&mut q, d, k);
        }
        // Matrice k × d (transposée de Q), ligne-major.
        let mut matrice = vec![0f32; k * d];
        for i in 0..d {
            for j in 0..k {
                matrice[j * d + i] = q[i * k + j];
            }
        }
        Ok(ProjectionApprise {
            dim_entree: d,
            dim_sortie: k,
            moyenne,
            matrice,
            lambda,
            trace,
            n_echantillon: n as u32,
            graine,
        })
    }

    fn projeter(&self, v: &[f32]) -> Vec<f32> {
        let centre: Vec<f32> = v.iter().zip(&self.moyenne).map(|(x, m)| x - m).collect();
        let mut out: Vec<f32> = self
            .matrice
            .chunks_exact(self.dim_entree)
            .map(|ligne| ligne.iter().zip(&centre).map(|(a, b)| a * b).sum::<f32>())
            .collect();
        normaliser(&mut out);
        out
    }

    /// Part de la variance totale portée par les composantes retenues, dans
    /// `[0, 1]`. MESURÉ : 0,967 sur 32 521 plongements réels 768-d.
    pub fn variance_expliquee(&self) -> f32 {
        if self.trace > 0.0 {
            (self.lambda.iter().sum::<f32>() / self.trace).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }

    /// Nombre de vecteurs effectivement utilisés pour l'ajustement.
    pub fn n_echantillon(&self) -> u32 {
        self.n_echantillon
    }

    /// Graine de l'initialisation.
    pub fn graine(&self) -> u64 {
        self.graine
    }

    /// Dimension d'entrée.
    pub fn dim_entree(&self) -> usize {
        self.dim_entree
    }

    /// Matrice `L2_DIM × dim_entree`, ligne-major (lecture seule).
    pub fn matrice(&self) -> &[f32] {
        &self.matrice
    }
}

pub(crate) fn normaliser(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 && n.is_finite() {
        for x in v.iter_mut() {
            *x /= n;
        }
    }
}

/// Gram-Schmidt modifié sur les colonnes de `q` (`d` lignes × `k` colonnes,
/// ligne-major `q[i * k + j]`).
fn gram_schmidt(q: &mut [f32], d: usize, k: usize) {
    for j in 0..k {
        for p in 0..j {
            let mut s = 0f32;
            for i in 0..d {
                s += q[i * k + j] * q[i * k + p];
            }
            for i in 0..d {
                q[i * k + j] -= s * q[i * k + p];
            }
        }
        let mut n = 0f32;
        for i in 0..d {
            n += q[i * k + j] * q[i * k + j];
        }
        let n = n.sqrt().max(1e-12);
        for i in 0..d {
            q[i * k + j] /= n;
        }
    }
}

/// Générateur déterministe (SplitMix64) : pas de dépendance, reproductible
/// sur toute plateforme — la matrice persistée doit se recalculer à l'identique.
pub(crate) struct SplitMix(pub(crate) u64);

impl SplitMix {
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f32 {
        ((self.next_u64() >> 11) as f64 / (1u64 << 53) as f64) as f32
    }
    pub(crate) fn gauss(&mut self) -> f32 {
        let u1 = self.unit().max(1e-7);
        let u2 = self.unit();
        (-2.0 * (u1 as f64).ln()).sqrt() as f32 * (2.0 * std::f32::consts::PI * u2).cos()
    }
}

/// Nom sûr pour un fichier : alphanumériques, `_` et `-` tels quels, tout le
/// reste en `%xx` (octets UTF-8). Injectif, donc deux index distincts ne se
/// marchent pas dessus.
fn echapper(s: &str) -> String {
    crate::file::zone::nom_sur(s)
}

/// Chemin du fichier de projection d'un index dans `dir`.
pub fn chemin(dir: &Path, label: &str, propriete: &str) -> PathBuf {
    dir.join(Projection::nom_fichier(label, propriete))
}

/// Jeux de données synthétiques partagés par les tests du crate (projection,
/// retrieval) : la recette « centres + 0,25·direction » du dépôt
/// (`agent_memory_migration.rs`), paramétrée en dimension.
#[cfg(test)]
pub(crate) mod jeux {
    use super::{SplitMix, normaliser};

    /// Direction unitaire tirée au hasard en dimension `dim`.
    pub(crate) fn direction(rng: &mut SplitMix, dim: usize) -> Vec<f32> {
        let mut v: Vec<f32> = (0..dim).map(|_| rng.gauss()).collect();
        normaliser(&mut v);
        v
    }

    /// `centres_n` centres unitaires ; le point `i` vaut
    /// normalise(centre[i % centres_n] + 0,25·direction). `n × dim` f32, ligne-major.
    pub(crate) fn structures(n: usize, centres_n: usize, dim: usize, graine: u64) -> Vec<f32> {
        let mut rng = SplitMix(graine);
        let centres: Vec<Vec<f32>> = (0..centres_n).map(|_| direction(&mut rng, dim)).collect();
        let mut out = Vec::with_capacity(n * dim);
        for i in 0..n {
            let u = direction(&mut rng, dim);
            let c = &centres[i % centres_n];
            let mut v: Vec<f32> = (0..dim).map(|k| c[k] + 0.25 * u[k]).collect();
            normaliser(&mut v);
            out.extend_from_slice(&v);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const D: usize = 768;

    fn structures(n: usize, centres_n: usize, graine: u64) -> Vec<f32> {
        jeux::structures(n, centres_n, D, graine)
    }

    fn repertoire_temporaire(nom: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "obrain-vecproj-{nom}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&p).expect("répertoire temporaire");
        p
    }

    #[test]
    fn identite_copie_et_complete_de_zeros() {
        let p = Projection::identite(5).unwrap();
        let out = p.projeter(&[1.0, 2.0, 3.0, 4.0, 5.0]).unwrap();
        assert_eq!(out.len(), L2_DIM);
        assert_eq!(&out[..5], &[1.0, 2.0, 3.0, 4.0, 5.0]);
        assert!(
            out[5..].iter().all(|&x| x == 0.0),
            "résultat attendu : zéros après la copie"
        );
        let plein = Projection::identite(L2_DIM).unwrap();
        let v: Vec<f32> = (0..L2_DIM).map(|i| i as f32).collect();
        assert_eq!(plein.projeter(&v).unwrap(), v);
    }

    #[test]
    fn identite_refuse_hors_bornes_et_mauvaise_longueur() {
        assert!(Projection::identite(0).is_err());
        assert!(Projection::identite(L2_DIM + 1).is_err());
        let p = Projection::identite(4).unwrap();
        assert!(p.projeter(&[1.0, 2.0, 3.0]).is_err());
    }

    #[test]
    fn ajuster_refuse_les_entrees_invalides() {
        assert!(
            ProjectionApprise::ajuster(&[0.0; 384 * 4], 384, 1).is_err(),
            "≤ L2_DIM ⇒ identité"
        );
        assert!(
            ProjectionApprise::ajuster(&[0.0; 768 + 1], 768, 1).is_err(),
            "longueur non multiple"
        );
        assert!(
            ProjectionApprise::ajuster(&[0.0; 768], 768, 1).is_err(),
            "un seul vecteur"
        );
    }

    #[test]
    fn ajuster_est_deterministe() {
        let ech = structures(600, 20, 7);
        let a = ProjectionApprise::ajuster(&ech, D, 42).unwrap();
        let b = ProjectionApprise::ajuster(&ech, D, 42).unwrap();
        assert_eq!(
            a.matrice(),
            b.matrice(),
            "même échantillon, même graine ⇒ même matrice au bit près"
        );
        let c = ProjectionApprise::ajuster(&ech, D, 43).unwrap();
        assert_ne!(
            a.matrice(),
            c.matrice(),
            "une autre graine change l'initialisation"
        );
    }

    #[test]
    fn composantes_orthonormees() {
        let ech = structures(600, 20, 11);
        let p = ProjectionApprise::ajuster(&ech, D, 1).unwrap();
        let m = p.matrice();
        let mut ecart_max = 0f32;
        for a in 0..L2_DIM {
            for b in a..L2_DIM {
                let s: f32 = (0..D).map(|i| m[a * D + i] * m[b * D + i]).sum();
                let cible = if a == b { 1.0 } else { 0.0 };
                ecart_max = ecart_max.max((s - cible).abs());
            }
        }
        assert!(ecart_max < 1e-3, "|QᵀQ − I| max = {ecart_max}");
    }

    #[test]
    fn variance_expliquee_sur_vecteurs_structures() {
        let ech = structures(2_000, 50, 5);
        let p = ProjectionApprise::ajuster(&ech, D, 9).unwrap();
        let ve = p.variance_expliquee();
        assert!(ve >= 0.90, "variance expliquée {ve} < 0,90");
        assert!(ve <= 1.0);
        assert_eq!(p.n_echantillon(), 2_000);
    }

    #[test]
    fn projeter_renormalise_et_respecte_la_dimension() {
        let ech = structures(300, 10, 3);
        let p = Projection::Apprise(ProjectionApprise::ajuster(&ech, D, 2).unwrap());
        let out = p.projeter(&ech[..D]).unwrap();
        assert_eq!(out.len(), L2_DIM);
        let n: f32 = out.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((n - 1.0).abs() < 1e-4, "norme {n}");
        assert!(p.projeter(&ech[..D - 1]).is_err());
    }

    #[test]
    fn aller_retour_octets_identique_au_bit_pres() {
        let ech = structures(300, 10, 21);
        let apprise = Projection::Apprise(ProjectionApprise::ajuster(&ech, D, 77).unwrap());
        let relue = Projection::depuis_octets(&apprise.en_octets()).unwrap();
        assert_eq!(relue, apprise);
        let identite = Projection::identite(128).unwrap();
        assert_eq!(
            Projection::depuis_octets(&identite.en_octets()).unwrap(),
            identite
        );
    }

    #[test]
    fn octet_corrompu_signature_ou_version_inconnue_sont_des_erreurs() {
        let ech = structures(300, 10, 21);
        let p = Projection::Apprise(ProjectionApprise::ajuster(&ech, D, 77).unwrap());
        let mut o = p.en_octets();
        let dernier = o.len() - 1;
        o[dernier] ^= 0x01;
        assert!(
            Projection::depuis_octets(&o).is_err(),
            "CRC doit détecter un octet du payload"
        );
        let mut o = p.en_octets();
        o[0] = b'X';
        assert!(Projection::depuis_octets(&o).is_err(), "signature");
        let mut o = p.en_octets();
        o[8] = 0xFF;
        o[9] = 0xFF;
        assert!(Projection::depuis_octets(&o).is_err(), "version");
        assert!(Projection::depuis_octets(&o[..10]).is_err(), "tronqué");
    }

    #[test]
    fn ecrire_lire_et_absent() {
        let dir = repertoire_temporaire("rw");
        assert_eq!(Projection::lire(&dir, "Doc", "emb").unwrap(), None);
        let ech = structures(300, 10, 4);
        let p = Projection::Apprise(ProjectionApprise::ajuster(&ech, D, 5).unwrap());
        p.ecrire(&dir, "Doc", "emb").unwrap();
        assert!(dir.join("substrate.vecproj.Doc.emb").is_file());
        assert!(
            !dir.join("substrate.vecproj.Doc.vecproj.tmp").exists(),
            "le temporaire est renommé"
        );
        assert_eq!(Projection::lire(&dir, "Doc", "emb").unwrap(), Some(p));
        // Un fichier corrompu sur disque est une erreur, pas un None.
        let chemin = dir.join("substrate.vecproj.Doc.emb");
        let mut o = std::fs::read(&chemin).unwrap();
        let dernier = o.len() - 1;
        o[dernier] ^= 0x01;
        std::fs::write(&chemin, o).unwrap();
        assert!(Projection::lire(&dir, "Doc", "emb").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn noms_de_fichiers_echappes_et_injectifs() {
        assert_eq!(
            Projection::nom_fichier("Doc", "emb"),
            "substrate.vecproj.Doc.emb"
        );
        let a = Projection::nom_fichier("a.b", "c");
        let b = Projection::nom_fichier("a", "b.c");
        assert_ne!(a, b, "l'échappement du point sépare les deux index");
        assert!(!Projection::nom_fichier("é/x", "y z").contains(['/', ' ']));
    }
}
