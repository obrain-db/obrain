//! L'identité d'un nœud — qui il est, et à quelle base il appartient.
//!
//! # Pourquoi une identité PERSISTÉE, et pas le nom d'hôte
//!
//! Le nom d'hôte ne suffit pas, et il ne suffira pas longtemps :
//!
//! * deux nœuds peuvent vivre sur la même machine (environnements, comptes,
//!   `systemd` distincts) — même nom d'hôte, deux nœuds ;
//! * un nom d'hôte se change, une machine se réinstalle, un conteneur se
//!   recrée : l'identité suivrait le hasard du parc ;
//! * surtout, **un nœud doit pouvoir être reconnu à travers ses redémarrages**.
//!   Un identifiant régénéré à chaque lancement rendrait impossible de dire
//!   « ce nœud-là a déjà fait ça » — donc impossible de tenir une tâche, une
//!   preuve ou une mémoire par nœud.
//!
//! L'identifiant est donc **engendré une fois** puis relu ; le nom d'hôte est
//! rapporté à côté, comme un fait daté, pas comme une identité.
//!
//! # L'autorité vient du système d'exploitation
//!
//! Le fichier d'identité vit sous le `--state-dir` du nœud et n'est lisible que
//! par son propriétaire. C'est la même doctrine que le CLI d'administration du
//! moteur : *« L'autorité du CLI vient du système d'exploitation : il agit sur un
//! fichier, et prétendre le contraire serait une comédie. »* Un verrou applicatif
//! par-dessus un fichier déjà protégé par ses permissions ne protégerait rien.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde_json::{json, Value};

/// L'identité durable d'un nœud.
#[derive(Debug, Clone)]
pub struct Identite {
    /// `node:<nom-hôte>-<8 hex>` — stable à travers les redémarrages.
    pub node_id: String,
    /// Nom d'hôte **au moment où l'identité a été créée** (fait daté).
    pub hostname: String,
    /// Nom d'hôte courant, relu à chaque démarrage (peut avoir changé).
    pub hostname_courant: String,
    /// Date de la première déclaration du nœud.
    pub premier_vu_ms: i64,
    /// La base commune à laquelle ce nœud se rattache, si elle est connue.
    pub base: Option<String>,
    /// Le fichier d'où vient cette identité — pour que `status` puisse le dire.
    pub fichier: PathBuf,
}

/// Horodatage en millisecondes depuis l'époque Unix.
pub fn maintenant_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Nom d'hôte de la machine.
///
/// `/proc/sys/kernel/hostname` d'abord : il ne dépend d'aucune variable
/// d'environnement, donc il reste juste dans un shell `ssh` non interactif — ce
/// qui est exactement le contexte d'un nœud déployé.
pub fn nom_hote() -> String {
    if let Ok(s) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let t = s.trim();
        if !t.is_empty() {
            return t.to_string();
        }
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "inconnu".to_string())
}

/// Hachage FNV-1a 64 bits — suffisant pour désambiguïser deux nœuds créés dans
/// la même milliseconde, et sans dépendance.
fn fnv1a(octets: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in octets {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// Répertoire d'état par défaut, aligné sur la convention XDG.
pub fn state_dir_defaut() -> PathBuf {
    if let Ok(d) = std::env::var("SELFD_STATE_DIR") {
        if !d.is_empty() {
            return PathBuf::from(d);
        }
    }
    if let Ok(x) = std::env::var("XDG_STATE_HOME") {
        if !x.is_empty() {
            return PathBuf::from(x).join("selfd");
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home).join(".local/state/selfd")
}

impl Identite {
    /// Charge l'identité du `state_dir`, ou l'engendre si c'est le premier
    /// démarrage du nœud.
    ///
    /// La base commune est **mise à jour** si elle est fournie au lancement :
    /// un nœud peut être rattaché à une autre base sans perdre son identité.
    pub fn charger_ou_creer(state_dir: &Path, base: Option<&str>) -> Result<Self> {
        std::fs::create_dir_all(state_dir)
            .with_context(|| format!("création de {}", state_dir.display()))?;
        let fichier = state_dir.join("node.json");
        let hote = nom_hote();

        let mut valeur: Value = match std::fs::read_to_string(&fichier) {
            Ok(txt) => serde_json::from_str(&txt).unwrap_or_else(|_| json!({})),
            Err(_) => json!({}),
        };

        let node_id = valeur
            .get("nodeId")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        let (node_id, premier_vu, hote_creation) = match node_id {
            Some(id) => (
                id,
                valeur.get("premierVuMs").and_then(|v| v.as_i64()).unwrap_or_else(maintenant_ms),
                valeur
                    .get("hostname")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&hote)
                    .to_string(),
            ),
            None => {
                // Engendré UNE fois : horodatage + pid + nom d'hôte, hachés.
                let t = maintenant_ms();
                let graine = format!("{hote}|{t}|{}", std::process::id());
                let huit = format!("{:08x}", (fnv1a(graine.as_bytes()) & 0xffff_ffff) as u32);
                (format!("node:{hote}-{huit}"), t, hote.clone())
            }
        };

        let base_effective = base
            .map(str::to_string)
            .or_else(|| valeur.get("base").and_then(|v| v.as_str()).map(str::to_string));

        valeur["schema"] = json!("selfd/node/1");
        valeur["nodeId"] = json!(node_id);
        valeur["hostname"] = json!(hote_creation);
        valeur["premierVuMs"] = json!(premier_vu);
        if let Some(b) = &base_effective {
            valeur["base"] = json!(b);
        }

        // Écriture ATOMIQUE : un fichier d'identité tronqué par une coupure
        // ferait engendrer un NOUVEAU nœud au redémarrage suivant — c'est-à-dire
        // perdre exactement ce que ce fichier existe pour garder.
        let tmp = state_dir.join("node.json.tmp");
        {
            let mut f = std::fs::File::create(&tmp)
                .with_context(|| format!("écriture de {}", tmp.display()))?;
            f.write_all(serde_json::to_vec_pretty(&valeur)?.as_slice())?;
            f.sync_all().ok();
        }
        std::fs::rename(&tmp, &fichier)
            .with_context(|| format!("installation de {}", fichier.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&fichier, std::fs::Permissions::from_mode(0o600));
        }

        Ok(Identite {
            node_id,
            hostname: hote_creation,
            hostname_courant: hote,
            premier_vu_ms: premier_vu,
            base: base_effective,
            fichier,
        })
    }

    /// L'identité telle qu'un client la lit.
    pub fn en_json(&self) -> Value {
        json!({
            "schema": "selfd/node/1",
            "nodeId": self.node_id,
            "hostname": self.hostname,
            "hostnameCourant": self.hostname_courant,
            "premierVuMs": self.premier_vu_ms,
            "base": self.base,
            "fichierIdentite": self.fichier.display().to_string(),
            // Si le nom d'hôte a changé depuis la création, le DIRE : sans cela,
            // un nœud renommé porterait un identifiant qui ne correspond plus à
            // rien de visible sur la machine.
            "hostnameChange": self.hostname != self.hostname_courant,
        })
    }
}
