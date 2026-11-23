#!/usr/bin/env python3
"""Build B — MMQ force pour TOUS les lots du micro-banc.

CE QUE `GGML_CUDA_FORCE_MMQ` NE FAIT PAS (verifie dans le code, pas suppose)
--------------------------------------------------------------------------
L'intuition est que `-DGGML_CUDA_FORCE_MMQ` suffit a obtenir MMQ partout. C'est faux, et l'ordre du
dispatch l'explique (ggml-cuda.cu:1795) :

    if (ne2 <= MMVQ_MAX_BATCH_SIZE) {                       // evalue EN PREMIER
        if (ggml_is_quantized(src0->type)) {
            ggml_cuda_mul_mat_vec_q(...); return;           // MMVQ gagne et rend la main
        }
    }
    if (ggml_cuda_should_use_mmq(...)) { ggml_cuda_mul_mat_q(...); return; }
                                         // ^ c'est ICI, et seulement ici, que FORCE_MMQ agit

Donc un build compile avec FORCE_MMQ continue d'utiliser MMVQ pour n <= 8 : il serait
comportementalement IDENTIQUE a build A dans la plage mesuree (n = 7, 8, 9, 10, 12, 16), et le
micro-banc n'aurait aucune mesure de MMQ a n = 7 et 8.

CE QUE CE SCRIPT FAIT
---------------------
Abaisse la frontiere du chemin vectoriel a 6 : le noyau vectoriel reste disponible pour n <= 8
(comportement nominal preserve pour les petits lots), mais la branche `ne2 <= 6` n'est plus prise a
nos lots, donc le dispatch tombe sur `should_use_mmq` — que le drapeau de compilation rend vrai.
Resultat : MMQ pour n = 7, 8, 9, 10, 12, 16.

`mmvf.cuh` suit par obligation : `static_assert(MMVQ_MAX_BATCH_SIZE == MMVF_MAX_BATCH_SIZE)`
(ggml-cuda.cu:1794). Aucun `switch` n'a besoin d'etre touche ici : on RETRECIT le domaine, on ne
l'etend pas — et le chemin MMVF (types non quantifies) n'est de toute facon pris que sur AMD.

Aucun poids, aucune quantification, aucun modele touche. Ce build n'est PAS livre.

USAGE
-----
    python3 experiments/force-mmq.py <chemin-du-depot-experimental>
    # puis configurer avec -DGGML_CUDA_FORCE_MMQ=ON -DCMAKE_CUDA_ARCHITECTURES=121
"""

import os
import sys


def patch(path, old, new, label):
    with open(path, encoding="utf-8") as f:
        txt = f.read()
    if new in txt:
        print("  deja patche : %s" % label)
        return
    if txt.count(old) != 1:
        raise SystemExit("ancre introuvable ou ambigue dans %s (%s)" % (path, label))
    with open(path, "w", encoding="utf-8") as f:
        f.write(txt.replace(old, new))
    print("  patche : %s" % label)


def main():
    repo = sys.argv[1] if len(sys.argv) > 1 else "."
    cuda = os.path.join(repo, "ggml", "src", "ggml-cuda")
    print("build B (MMQ force sur la plage du banc)")
    patch(os.path.join(cuda, "mmvq.cuh"),
          "#define MMVQ_MAX_BATCH_SIZE 8",
          "#define MMVQ_MAX_BATCH_SIZE 6 // [obrain EXP force-mmq] 8 -> 6 : la branche vectorielle n'est\n"
          "                             // plus prise a n >= 7, donc le dispatch tombe sur MMQ (FORCE_MMQ)",
          "mmvq.cuh : MMVQ_MAX_BATCH_SIZE 8 -> 6")
    patch(os.path.join(cuda, "mmvf.cuh"),
          "#define MMVF_MAX_BATCH_SIZE 8",
          "#define MMVF_MAX_BATCH_SIZE 6 // [obrain EXP force-mmq] doit suivre MMVQ (static_assert)",
          "mmvf.cuh : MMVF_MAX_BATCH_SIZE 8 -> 6")
    print("termine. Rappel : compiler avec -DGGML_CUDA_FORCE_MMQ=ON, sinon ce build ne change rien.")


if __name__ == "__main__":
    main()
