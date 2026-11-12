"""GQL : exécution, lignes itérables, paramètres."""


def _db_personnes(obrain):
    db = obrain.ObrainDB()
    db.execute("INSERT (:Person {name: 'Alix', age: 30})")
    db.execute("INSERT (:Person {name: 'Gus', age: 25})")
    db.execute("INSERT (:Person {name: 'Vincent', age: 41})")
    return db


def test_match_retourne_des_lignes_nommees():
    import obrain

    db = _db_personnes(obrain)
    result = db.execute("MATCH (p:Person) RETURN p.name AS name, p.age AS age ORDER BY name")
    assert result.columns == ["name", "age"]
    assert result.to_list() == [
        {"name": "Alix", "age": 30},
        {"name": "Gus", "age": 25},
        {"name": "Vincent", "age": 41},
    ]


def test_iteration_des_lignes():
    import obrain

    db = _db_personnes(obrain)
    result = db.execute("MATCH (p:Person) RETURN p.name AS name ORDER BY name")
    noms = [row["name"] for row in result]
    assert noms == ["Alix", "Gus", "Vincent"]


def test_parametres_nommes():
    import obrain

    db = _db_personnes(obrain)
    result = db.execute(
        "MATCH (p:Person) WHERE p.age > $min RETURN p.name AS name ORDER BY name",
        {"min": 28},
    )
    assert result.to_list() == [{"name": "Alix"}, {"name": "Vincent"}]


def test_parametre_chaine_n_est_pas_interprete_comme_gql():
    import obrain

    db = _db_personnes(obrain)
    # Un paramètre contenant du texte GQL reste une valeur, jamais du code.
    result = db.execute(
        "MATCH (p:Person) WHERE p.name = $n RETURN p.name AS name",
        {"n": "Alix' OR 1=1 --"},
    )
    assert result.to_list() == []


def test_insert_puis_relecture_de_relation():
    import obrain

    db = _db_personnes(obrain)
    db.execute(
        "MATCH (a:Person {name: 'Alix'}), (b:Person {name: 'Gus'}) "
        "INSERT (a)-[:KNOWS]->(b)"
    )
    result = db.execute(
        "MATCH (a)-[:KNOWS]->(b) RETURN a.name AS a, b.name AS b"
    )
    assert result.to_list() == [{"a": "Alix", "b": "Gus"}]
