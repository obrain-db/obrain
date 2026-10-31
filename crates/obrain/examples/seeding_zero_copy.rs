//! Démo O1 → E2 : seeding spéculatif alimenté par une FactView SANS copie.
//!
//! Reproduit la couture exacte avec `engine-spec` (rustorch-engine, brique
//! E2) sans dépendance croisée : son trait `DraftProposer` expose
//! `seed(&[Token])`, agnostique — une suite de tokens opaque. Ici, le texte
//! d'un fait du graphe est TOKENISÉ DIRECTEMENT depuis la vue empruntée aux
//! pages mmap du substrate : aucun `String` intermédiaire, aucune
//! sérialisation JSON, aucun parse. Le seul octet écrit est le token produit.
//!
//! ```text
//! substrate (mmap) ──vue &str──▶ tokenizer ──&[Token]──▶ proposer.seed()
//!            zéro copie              stream               (engine-spec E2)
//! ```
//!
//! Lancer : `cargo run -p obrain --example seeding_zero_copy`

use obrain::{NodeId, ObrainDB};

/// Miroir minimal de `engine_spec::Token` (llama_token / GGUF).
type Token = i32;

/// Miroir minimal de `engine_spec::DraftProposer::seed` — la démo prouve la
/// couture, pas le proposeur (voir rustorch-engine crates/engine-spec).
struct ProposeurDemo {
    indexes: usize,
}

impl ProposeurDemo {
    fn seed(&mut self, tokens: &[Token]) {
        self.indexes += tokens.len();
    }
}

/// Tokenizer jouet PAR OCTET, streaming : consomme un `&str` emprunté sans
/// jamais le posséder. Un vrai BPE (rustorch-tokenizer) a la même forme :
/// `encode(&str) -> Vec<Token>` — l'entrée reste une vue.
fn tokenise_streaming(texte: &str, sortie: &mut Vec<Token>) {
    sortie.clear();
    sortie.extend(texte.as_bytes().iter().map(|&b| b as Token));
}

fn main() {
    let tmp = std::env::temp_dir().join(format!("demo-seeding-{}.obrain", std::process::id()));
    // 1) Un graphe avec des faits (texte court → PropsZone, long → blobcol).
    {
        let db = ObrainDB::open_substrate(&tmp).expect("création");
        let session = db.session();
        session
            .execute(
                "INSERT (:Fait {sujet: 'rustorch', texte: \
                 'le moteur rustorch-engine lit le graphe obrain in-process, \
                 par vues memoire, sans FFI ni serialisation'})",
            )
            .expect("insert fait 1");
        let long = "chaque brouillon speculatif cite verbatim les faits du graphe. ".repeat(6); // > 256 o → colonne blob
        session
            .execute(&format!(
                "INSERT (:Fait {{sujet: 'seeding', texte: '{long}'}})"
            ))
            .expect("insert fait 2");
    }

    // 2) Réouverture LECTURE SEULE partagée — le régime du moteur d'inférence.
    let db = ObrainDB::open_read_only(&tmp).expect("réouverture RO");
    let session = db.session();
    let mut proposeur = ProposeurDemo { indexes: 0 };
    let mut tampon: Vec<Token> = Vec::with_capacity(1024); // buffer réutilisé

    for id in 0..16u64 {
        // La vue : un &str emprunté aux pages du substrate, sous garde de
        // lecture. Pas de Value, pas d'ArcStr, pas de JSON.
        let Some(fait) = session.fact_view(NodeId(id), "texte") else {
            continue;
        };
        tokenise_streaming(&fait.text, &mut tampon);
        proposeur.seed(&tampon);
        println!(
            "fait {:?} ({:?}, {} o) → {} tokens seedés sans copie",
            fait.node,
            fait.text.source(),
            fait.text.len(),
            tampon.len()
        );
        // `fait` est droppé ici : la garde de lecture est relâchée.
    }

    println!(
        "total: {} tokens indexés par le proposeur",
        proposeur.indexes
    );
    let _ = std::fs::remove_dir_all(&tmp);
}
