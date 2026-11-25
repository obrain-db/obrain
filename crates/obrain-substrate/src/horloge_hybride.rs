//! Horloge hybride (HLC) pour la convergence CRDT des écritures partagées (#334).
//!
//! Chaque écriture de propriété sera estampillée d'une [`Estampille`]
//! (composante physique + compteur logique). Deux estampilles forment un ordre
//! total : toute écriture qui a *observé* une estampille distante en porte une
//! strictement supérieure, ce qui permet de décider la dernière écriture
//! gagnante (LWW) au rejeu, quel que soit l'ordre de réception.
use serde::{Deserialize, Serialize};

/// Une estampille HLC : `physique` (micros depuis l'époque) puis `logique`
/// (compteur pour départager des événements à la même composante physique).
/// L'ordre est d'abord par `physique`, puis par `logique`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Estampille {
    pub physique: i64,
    pub logique: u32,
}

/// État d'une horloge hybride locale (une par écrivain).
#[derive(Debug, Clone, Copy, Default)]
pub struct HorlogeHybride {
    physique: i64,
    logique: u32,
}

impl Estampille {
    /// Encode en u64 pour le stockage on-disk : 48 bits de `physique` (positif,
    /// millis) + 16 bits de `logique`. Suffit pour la convergence LWW.
    pub fn pack(self) -> u64 {
        ((self.physique.max(0) as u64) << 16) | (self.logique as u64 & 0xFFFF)
    }

    pub fn depaqueter(v: u64) -> Self {
        Estampille {
            physique: (v >> 16) as i64,
            logique: (v & 0xFFFF) as u32,
        }
    }
}

impl HorlogeHybride {
    pub fn nouvelle() -> Self {
        Self::default()
    }

    /// Progression locale (événement d'écriture). `physique_instant` est
    /// l'horloge murale du moment, injectée pour rendre les tests déterministes.
    pub fn maintenant(&mut self, physique_instant: i64) -> Estampille {
        let plafond = physique_instant.max(self.physique);
        if plafond == self.physique {
            // L'horloge murale n'a pas dépassé notre dernière composante : on
            // avance le compteur logique pour garantir la stricte croissance.
            self.logique += 1;
        } else {
            self.physique = plafond;
            self.logique = 0;
        }
        Estampille {
            physique: self.physique,
            logique: self.logique,
        }
    }

    /// Observation d'une estampille distante (réception). L'horloge locale est
    /// portée strictement au-delà du distant : toute écriture ultérieure dominera.
    pub fn observer(&mut self, distant: Estampille, physique_instant: i64) {
        let plafond = physique_instant.max(self.physique).max(distant.physique);
        if plafond == self.physique && plafond == distant.physique {
            self.logique = self.logique.max(distant.logique) + 1;
        } else if plafond == distant.physique {
            self.logique = distant.logique + 1;
        } else if plafond == self.physique {
            self.logique += 1;
        } else {
            self.logique = 0;
        }
        self.physique = plafond;
    }

    pub fn derniere(&self) -> Estampille {
        Estampille {
            physique: self.physique,
            logique: self.logique,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Estampille, HorlogeHybride};

    #[test]
    fn maintenant_est_strictement_croissant_a_physique_fixe() {
        let mut h = HorlogeHybride::nouvelle();
        let a = h.maintenant(1_000);
        let b = h.maintenant(1_000);
        let c = h.maintenant(1_000);
        assert!(
            a < b && b < c,
            "même composante physique, logique croissante"
        );
        assert_eq!((a.physique, a.logique), (1_000, 0));
        assert_eq!((b.physique, b.logique), (1_000, 1));
        assert_eq!((c.physique, c.logique), (1_000, 2));
    }

    #[test]
    fn maintenant_reinitialise_le_logique_quand_la_physique_avance() {
        let mut h = HorlogeHybride::nouvelle();
        h.maintenant(1_000);
        h.maintenant(1_000);
        let e = h.maintenant(2_000);
        assert_eq!((e.physique, e.logique), (2_000, 0));
    }

    #[test]
    fn observer_porte_l_horloge_au_dela_du_distant() {
        let mut h = HorlogeHybride::nouvelle();
        h.maintenant(1_000);
        let distant = Estampille {
            physique: 5_000,
            logique: 7,
        };
        h.observer(distant, 1_000);
        // La prochaine écriture locale doit dominer strictement le distant.
        let e = h.maintenant(1_000);
        assert!(e > distant, "l'écriture post-observation domine le distant");
        assert_eq!(e.physique, 5_000);
    }

    #[test]
    fn ordre_total_physique_puis_logique() {
        let a = Estampille {
            physique: 1,
            logique: 9,
        };
        let b = Estampille {
            physique: 2,
            logique: 0,
        };
        let c = Estampille {
            physique: 2,
            logique: 1,
        };
        assert!(a < b && b < c);
        assert_eq!(c.max(a), c);
    }

    #[test]
    fn derniere_rend_la_derniere_estampille() {
        let mut h = HorlogeHybride::nouvelle();
        assert_eq!(
            h.derniere(),
            Estampille {
                physique: 0,
                logique: 0
            }
        );
        let e = h.maintenant(1_000);
        assert_eq!(h.derniere(), e);
    }

    #[test]
    fn estampille_serialise_et_deserialise() {
        let e = Estampille {
            physique: 42,
            logique: 7,
        };
        let octets =
            bincode::serde::encode_to_vec(e, bincode::config::standard()).expect("sérialisation");
        let (d, _): (Estampille, usize) =
            bincode::serde::decode_from_slice(&octets, bincode::config::standard())
                .expect("désérialisation");
        assert_eq!(d, e);
    }

    #[test]
    fn observer_branche_egalite_physique() {
        let mut h = HorlogeHybride::nouvelle();
        h.maintenant(1_000);
        let distant = Estampille {
            physique: 1_000,
            logique: 5,
        };
        h.observer(distant, 500);
        // plafond == self.physique == distant.physique : logique = max(0, 5) + 1.
        assert_eq!(
            h.derniere(),
            Estampille {
                physique: 1_000,
                logique: 6
            }
        );
    }

    #[test]
    fn observer_branche_self_en_avance() {
        let mut h = HorlogeHybride::nouvelle();
        h.maintenant(1_000);
        h.maintenant(1_000);
        let distant = Estampille {
            physique: 500,
            logique: 9,
        };
        h.observer(distant, 500);
        // plafond == self.physique (local en avance) : logique += 1.
        assert_eq!(
            h.derniere(),
            Estampille {
                physique: 1_000,
                logique: 2
            }
        );
    }

    #[test]
    fn deux_ecrivains_qui_s_observent_convergent() {
        let mut x = HorlogeHybride::nouvelle();
        let mut y = HorlogeHybride::nouvelle();
        // x écrit, y l'observe, puis chacun écrit : l'estampille de x domine y,
        // et réciproquement — un ordre total unique émerge.
        let x1 = x.maintenant(1_000);
        y.observer(x1, 900);
        let y1 = y.maintenant(1_000);
        x.observer(y1, 1_100);
        let x2 = x.maintenant(1_100);
        assert!(x2 > y1, "x a observé y1 et écrit après");
    }
}
