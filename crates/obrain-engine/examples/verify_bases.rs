//! Opens a database and prints basic node/edge counts for verification.

use obrain_engine::ObrainDB;

fn main() {
    let bases = [
        "/CHEMIN/UTILISATEUR/.obrain/db/po",
        "/CHEMIN/UTILISATEUR/.obrain/db/megalaw",
        "/CHEMIN/UTILISATEUR/.obrain/db/wikipedia",
    ];
    for path in &bases {
        print!("Opening {path}... ");
        match ObrainDB::open(path) {
            Ok(db) => {
                let store = db.store();
                let n = store.node_count();
                let e = store.edge_count();
                println!("OK nodes={n} edges={e}");
            }
            Err(err) => println!("FAIL: {err}"),
        }
    }
}
