//! Premier test natif de composition : structure partagée via simplexes.
//! ABC={1,2,3} et ABD={1,2,4} partagent la sous-structure {1,2}. On vérifie
//! la persistance après fermeture/réouverture (pas un round-trip mémoire).

use obrain_substrate::record::simplex_kind::TRIANGLE;
use obrain_substrate::{SubstrateFile, SyncMode, Writer};

#[test]
fn composition_native_fermeture_reouverture() {
    let td = tempfile::tempdir().unwrap();
    let path = td.path().join("kb");
    let (abc, abd);
    {
        let sub = SubstrateFile::create(&path).unwrap();
        let w = Writer::new(sub, SyncMode::EveryCommit).unwrap();
        let (id, _) = w.upsert_simplex(TRIANGLE, 0, 0, &[1, 2, 3], &[]).unwrap();
        abc = id;
        let (id, _) = w.upsert_simplex(TRIANGLE, 0, 0, &[1, 2, 4], &[]).unwrap();
        abd = id;
        w.commit().unwrap();
        w.msync_zones().unwrap();
        w.wal().fsync().unwrap();
    }
    // Suppression d'ABD : ne doit pas détruire ABC (ni la sous-structure {1,2}).
    {
        let sub = SubstrateFile::open(&path).unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        assert!(w.tombstone_simplex(abd).unwrap(), "tombstone ABD");
        w.commit().unwrap();
        w.msync_zones().unwrap();
        w.wal().fsync().unwrap();
    }
    // Réouverture : ABC survit avec ses membres, ABD est absent.
    {
        let sub = SubstrateFile::open(&path).unwrap();
        let w = Writer::new(sub, SyncMode::Never).unwrap();
        let v = w
            .get_simplex(abc)
            .unwrap()
            .expect("ABC présent après réouverture");
        assert_eq!(v.members, vec![1, 2, 3], "membres d'ABC conservés");
        assert!(
            w.get_simplex(abd).unwrap().is_none(),
            "ABD supprimé durablement"
        );
    }
}
