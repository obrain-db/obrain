# Vérification multi-langages : indexeur natif M6 vs indexeur C

Date : 2026-09-07. Méthode : corpus `/tmp/m6-multilang` (11 fichiers, 11
langages : ada, clojure, cobol, dart, erlang, fortran, powershell, scss,
sql, swift, vue) indexé par les DEUX indexeurs sur le MÊME arbre.
Référence C : `codebase-memory-mcp cli index_repository` → project
`tmp-m6-multilang`. M6 : build_registry() (146 langages).

## Comparatif final (après résolveurs de parité)

| Fichier | C (référence) | M6 | Écart |
|---|---|---|---|
| sample.clj | Module:myapp.core, Fn:helper, Struct:Point, Fn:area | **identique** | 0 |
| sample.sql | Table:users, Fn:add_one | **Table:users + Field id/name + Fn:add_one** | 0 (superset colonnes) |
| sample.cob | Fn:HELLO | **Fn:HELLO + Var:WS-NUM** | 0 (superset) |
| sample.scss | Var:$primary-color, Fn:flex-center | **identique** | 0 |
| sample.ps1 | Fn:Get-Helper | identique | 0 |
| sample.erl | — | Fn:helper | M6 ⊃ C |
| sample.dart | Method:dist | Class:Point + Field x/y + Method:dist + Fn:helper | M6 ⊃ C |
| sample.swift | Var x/y, Method:distance | Class:Point + Field x/y + Method:distance + Fn:helper | M6 ⊃ C |
| sample.f90 | Fn:double_it | Fn:helper + Fn:double_it + Var a/x | M6 ⊃ C |
| sample.adb | non indexé (découverte) | Fn:Push + Fn:Pop | M6 ⊃ C |
| sample.vue | — | — | égal (scripts embarqués) |

**Résultat : 0 symbole de la référence C manquant** — chaque symbole C est
couvert, la plupart des fichiers en superset (classes Point que le C rate,
procédures .adb, variables fortran, champs dart/swift).

## Résolveurs ajoutés pour la parité (commits)

1. SQL (`25614779`) : special_name sql → CREATE TABLE users = Table,
   CREATE FUNCTION = Function (nom dans object_reference).
2. Fortran (`25614779`) : variable_name saute les TYPES (TYPE_KINDS) — plus
   de faux Variable:integer.
3. Clojure (`128d069b`, `ea65f3d6`) : special_name (parenthèse en 1er
   enfant, mot-clé en 2e, nom en 3e) + clojure_form_kind (ns → Module,
   defrecord → Struct, defn → Function).
4. Cobol (`a18a62f5`) : program_name (programme) + entry_name (données) +
   override data_description (grammaire actuelle vs data_description_entry C)
   + variable_name appelle special_name.
5. Erlang (`7231d23b`) : function_clause → premier enfant atom.
6. SCSS (`e69742c8`) : variables $ (filtre propriétés CSS).
7. Swift/Dart (`58d424e2`) : champs extraits sans bruit de type
   (find_first_identifier enrichi + skip type + field/variable exclusifs).
8. class_kind (`ea65f3d6`) : labels par type de nœud (Struct/Interface/
   Enum/Table/Type/Class).

## Conclusion

Parité totale atteinte sur le corpus de test : M6 couvre 100 % des symboles
de la référence C, en superset sur la majorité des fichiers. Les labels
(Module/Struct/Table/Class) correspondent au C.
