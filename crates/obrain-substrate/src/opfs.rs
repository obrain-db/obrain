//! Journal sur OPFS (navigateur, worker dédié) : [`FichierJournal`] réalisé par un
//! `FileSystemSyncAccessHandle`.
//!
//! L'exclusivité du handle est l'exclusivité de l'écrivain : OPFS n'en accorde qu'un
//! par fichier, donc deux onglets ne peuvent pas écrire le même store. Les appels
//! sont synchrones (c'est le propre du handle), ce qui permet au moteur de rester
//! synchrone. Seule l'OUVERTURE est asynchrone, et elle se fait avant le moteur.
//!
//! Toute erreur JS remonte en `io::Error` : aucune exception ne doit devenir une
//! trappe wasm, qui abattrait l'instance entière.

use std::io;

use wasm_bindgen::prelude::*;

use crate::es::FichierJournal;

#[wasm_bindgen(inline_js = r#"
const acces = new Map();
let prochain = 1;

export async function opfs_ouvrir(nom) {
  const racine = await navigator.storage.getDirectory();
  const fichier = await racine.getFileHandle(nom, { create: true });
  const h = await fichier.createSyncAccessHandle();
  const id = prochain++;
  acces.set(id, h);
  return id;
}

function poigne(id) {
  const h = acces.get(id);
  if (!h) throw new Error("handle OPFS fermé ou inconnu : " + id);
  return h;
}

export function opfs_taille(id) { return poigne(id).getSize(); }

export function opfs_lire(id, debut, longueur) {
  const tampon = new Uint8Array(longueur);
  const n = poigne(id).read(tampon, { at: debut });
  return tampon.subarray(0, n);
}

export function opfs_ecrire(id, octets, debut) {
  return poigne(id).write(octets, { at: debut });
}

export function opfs_tronquer(id, longueur) { poigne(id).truncate(longueur); }

export function opfs_synchroniser(id) { poigne(id).flush(); }

export function opfs_fermer(id) {
  const h = acces.get(id);
  if (h) { h.close(); acces.delete(id); }
}
"#)]
extern "C" {
    #[wasm_bindgen(catch)]
    async fn opfs_ouvrir(nom: &str) -> Result<JsValue, JsValue>;
    #[wasm_bindgen(catch)]
    fn opfs_taille(id: u32) -> Result<f64, JsValue>;
    #[wasm_bindgen(catch)]
    fn opfs_lire(id: u32, debut: f64, longueur: u32) -> Result<Vec<u8>, JsValue>;
    #[wasm_bindgen(catch)]
    fn opfs_ecrire(id: u32, octets: &[u8], debut: f64) -> Result<u32, JsValue>;
    #[wasm_bindgen(catch)]
    fn opfs_tronquer(id: u32, longueur: f64) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    fn opfs_synchroniser(id: u32) -> Result<(), JsValue>;
    #[wasm_bindgen(catch)]
    fn opfs_fermer(id: u32) -> Result<(), JsValue>;
}

fn erreur_js(e: JsValue) -> io::Error {
    io::Error::other(format!("OPFS : {e:?}"))
}

/// Journal d'un fichier OPFS. Fermé (handle libéré) quand il est détruit.
#[derive(Debug)]
pub struct JournalOpfs {
    id: u32,
}

impl JournalOpfs {
    /// Ouvre (ou crée) le fichier `nom` à la racine OPFS et en prend le handle.
    /// Échoue si un autre handle sur ce fichier existe déjà : c'est l'exclusivité.
    pub async fn ouvrir(nom: &str) -> io::Result<Self> {
        let id = opfs_ouvrir(nom).await.map_err(erreur_js)?;
        let id = id
            .as_f64()
            .ok_or_else(|| io::Error::other("OPFS : identifiant de handle invalide"))?
            as u32;
        Ok(Self { id })
    }
}

impl Drop for JournalOpfs {
    fn drop(&mut self) {
        // Une erreur de fermeture n'a pas de sens ici : le handle disparaît de toute façon.
        let _ = opfs_fermer(self.id);
    }
}

impl FichierJournal for JournalOpfs {
    fn ajouter(&mut self, buf: &[u8]) -> io::Result<usize> {
        let fin = self.taille()?;
        opfs_ecrire(self.id, buf, fin as f64).map_err(erreur_js)?;
        Ok(buf.len())
    }

    fn taille(&self) -> io::Result<u64> {
        opfs_taille(self.id).map(|t| t as u64).map_err(erreur_js)
    }

    fn tronquer(&mut self, longueur: u64) -> io::Result<()> {
        opfs_tronquer(self.id, longueur as f64).map_err(erreur_js)
    }

    fn lire_a(&self, debut: u64, buf: &mut [u8]) -> io::Result<()> {
        let octets = opfs_lire(self.id, debut as f64, buf.len() as u32).map_err(erreur_js)?;
        if octets.len() != buf.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "OPFS : lecture plus courte que demandée",
            ));
        }
        buf.copy_from_slice(&octets);
        Ok(())
    }

    fn rembobiner(&mut self) -> io::Result<()> {
        // Les ajouts et lectures portent un décalage explicite : rien à faire.
        Ok(())
    }

    fn synchroniser(&mut self, _tout: bool) -> io::Result<()> {
        // `flush` rend durables les données ET les métadonnées du fichier.
        opfs_synchroniser(self.id).map_err(erreur_js)
    }
}
