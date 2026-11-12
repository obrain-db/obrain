# obrain-db — top-level Makefile
#
# Cibles de benchs. Pour le développement courant, préférer les commandes
# `cargo` par crate.
#
# (2026-11-11) Les anciennes cibles `bench-baseline*`, `bench-substrate*`,
# `bench-compare`, `bench-smoke` et `check-substrate` sont retirées : elles
# lançaient un harnais `bench/substrate` qui n'existe plus dans le dépôt, sur
# des bases désignées par `PO_DB` / `MEGALAW_DB` (avec, jusqu'à l'incident du
# 2026-11-11, un défaut sous `$(HOME)`). Les bancs sur bases réelles passent
# désormais par `OBRAIN_BENCH_DB_*` et une copie : voir docs/BENCHMARKS.md,
# « Bancs sur bases réelles ».

BENCH_OUT  := target/bench

.PHONY: bench-v001 bench-v001-save bench-v001-compare
# Les quatre benchs de référence de la v0.0.1 (voir docs/BENCHMARKS.md).
bench-v001:
	scripts/bench-v001.sh run

# Ligne de base Criterion nommée (défaut : sha court) et comparaison avec seuil
# (BENCH_SEUIL, 10 % par défaut) : `make bench-v001-compare BASE=<nom>` sort 1
# si une mesure a régressé.
bench-v001-save:
	scripts/bench-v001.sh save $(BASE)
bench-v001-compare:
	scripts/bench-v001.sh compare $(BASE)

.PHONY: help
help:
	@echo "obrain-db top-level targets:"
	@echo "  make bench-v001              — Les 4 benchs de référence v0.0.1 (docs/BENCHMARKS.md)"
	@echo "  make bench-v001-save BASE=x  — Ligne de base Criterion x"
	@echo "  make bench-v001-compare BASE=x — Comparaison à x, seuil BENCH_SEUIL (10 %)"
	@echo "  make bench-clean             — Remove target/bench/"
	@echo ""
	@echo "Bancs sur bases réelles : docs/BENCHMARKS.md (section « Bancs sur bases réelles »)."

.PHONY: bench-clean
bench-clean:
	rm -rf $(BENCH_OUT)
