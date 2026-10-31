//! Sonde GQL J4 (scratch) : execute une requete sous une vue confinee.
use obrain_engine::ObrainDB;
fn main() {
    let mut a = std::env::args().skip(1);
    let chemin = a.next().expect("sonde_j4 <store> <tenant> <gql>");
    let tenant: u32 = a.next().unwrap().parse().unwrap();
    let gql = a.next().unwrap();
    let d = ObrainDB::open_read_only(&chemin).expect("open ro");
    d.avec_principal(tenant).expect("principal");
    let r = d.session().execute(&gql).expect("execute");
    for ligne in r.rows {
        println!("{ligne:?}");
    }
}
