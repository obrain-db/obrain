//! PRINCIPAL COTE SUBSTRAT — jalon 3 du RFC principal-natif-v2.
//!
//! Le type minimal que le substrat comprend : UN tenant. Pas de roles, pas
//! de nom de lecteur — ces champs appartiennent au `Principal` de session
//! (jalon 4, obrain-core/obrain-engine), qui DERIVERA un `PrincipalSubstrat`
//! et le posera ici. C'est LE point d'entree que le jalon 4 branche :
//!
//! ```text
//!   Session::session_as(Principal { tenant, .. })      (jalon 4)
//!         └─> SubstrateStore::avec_principal(PrincipalSubstrat::nouveau(tenant)?)
//!                   └─> toutes les primitives de lecture filtrent (jalon 3)
//! ```
//!
//! ## Immuabilite — la lecon d'`access_context`
//!
//! `access_context` (obrain-engine, session/mod.rs) est un
//! `Mutex<Option<Arc<AccessContext>>>` avec un setter public : une identite
//! MUTABLE apres construction, que le middleware doit *penser* a poser —
//! une convention, pas une garantie. Le `PrincipalSubstrat` prend le
//! contre-pied exact : il se pose UNE fois (`OnceLock`), il n'existe AUCUN
//! setter qui l'ecrase ni ne l'efface, et le type lui-meme n'expose aucune
//! mutation. Un handle de store est donc soit vierge (comportement
//! historique, tout visible sauf la sentinelle), soit lie a UN tenant pour
//! toute sa vie.
//!
//! ## Pourquoi un type nomme et pas un `Option<u32>` nu
//!
//! * le constructeur REFUSE la sentinelle (`u32::MAX`) — un `u32` nu ne
//!   peut pas ; sans ce refus, poser `u32::MAX` comme principal rendrait
//!   la sentinelle visible d'elle-meme (`tenant == portee`), l'inverse
//!   exact du fail-closed A5 ;
//! * les sites d'appel se lisent (`avec_principal(PrincipalSubstrat...)`
//!   dit ce qu'il fait ; `avec_principal(7)` ne dit rien) ;
//! * le jalon 4 etendra la DERIVATION (Principal de session →
//!   PrincipalSubstrat) sans toucher au substrat : le type nomme est le
//!   point de couture.

use crate::error::{SubstrateError, SubstrateResult};
use crate::record::tenant;

/// L'identite de lecture posee sur un `SubstrateStore` : UN tenant.
///
/// Semantique de visibilite (voir [`tenant::visible`], LE predicat) :
/// un noeud est visible si `noeud.tenant == principal.tenant` OU
/// `noeud.tenant == tenant::PUBLIC`. La sentinelle `u32::MAX` n'est
/// visible de personne. `PrincipalSubstrat { tenant_id: 0 }` est legal :
/// un principal PUBLIC ne voit que le public — utile aux services qui ne
/// doivent jamais voir de donnees tenantees.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrincipalSubstrat {
    tenant_id: u32,
}

impl PrincipalSubstrat {
    /// Construit un principal pour `tenant_id`. Refuse la sentinelle
    /// `u32::MAX` : elle est reservee aux noeuds non migres et ne doit
    /// jamais devenir une portee de lecture.
    pub fn nouveau(tenant_id: u32) -> SubstrateResult<Self> {
        if tenant_id == tenant::SENTINELLE_NON_MIGRE {
            return Err(SubstrateError::PrincipalIrrecevable(tenant_id));
        }
        Ok(Self { tenant_id })
    }

    /// Le tenant du principal. Lecture seule — le type n'expose aucune
    /// mutation, c'est verifie par un test de compilation (jalon 4 en
    /// exigera autant du `Principal` de session).
    #[inline]
    pub fn tenant_id(&self) -> u32 {
        self.tenant_id
    }
}
