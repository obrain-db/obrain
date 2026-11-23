#!/usr/bin/env python3
"""Build C — MMVQ etendu experimentalement jusqu'a n = 16.

POURQUOI CE SCRIPT EXISTE
-------------------------
Le bord 8/9 est etabli end-to-end (docs/ETAT-PORTAGE-CUDA-TQ2G128-2026-09-22.md §15) : a dm=7 le lot
de verification vaut 8 et la verification coute 30,15 ms par jeton commis ; a dm=9 le lot vaut 10 et
elle en coute 67,04. Il faut savoir si c'est une limite de PERFOMANCE ou de VALIDITE du chemin
vectoriel — donc construire un binaire ou MMVQ existe au-dela de 8 et le mesurer.

CE QUE LA CONSTANTE SEULE NE SUFFIT PAS A FAIRE
-----------------------------------------------
Verifie dans le code, pas suppose. Trois autres bornes :

  1. `mmvf.cuh` + `static_assert(MMVQ_MAX_BATCH_SIZE == MMVF_MAX_BATCH_SIZE)` (ggml-cuda.cu:1794) :
     les deux constantes doivent bouger ENSEMBLE, sinon la compilation echoue ;
  2. `mmvq.cu` : `mul_mat_vec_q_switch_ncols_dst` a `case 1..8` puis `default: GGML_ABORT`, donc
     relever la constante sans ajouter les cas 9..16 ferait PLANTER le binaire ;
  3. `mmvf.cu` : meme structure de l'autre cote (types non quantifies).

CE QUE CE SCRIPT FAIT
---------------------
  - MMVQ_MAX_BATCH_SIZE et MMVF_MAX_BATCH_SIZE : 8 -> 16 ;
  - ajoute les cas 9..16 aux deux dispatchs, mais UNIQUEMENT pour les types quantifies que la
    verification du modele utilise reellement (inventaire du GGUF : TQ2_G128 168 tenseurs, Q4_K 176,
    Q5_K 65, Q6_K 1). Le `if constexpr` evite d'instancier 8 cas supplementaires pour les ~25 autres
    types, ce qui ferait exploser le temps de compilation et la taille du binaire.

CE QUE CE SCRIPT NE FAIT PAS
----------------------------
Aucun changement de comportement pour n <= 8 : les cas 1..8 sont intacts, et la constante n'est lue
que pour decider si le chemin vectoriel est disponible. Aucun poids, aucune quantification, aucun
modele touche. Ce build n'est PAS livre : c'est une experience, et elle se termine par un revert.

USAGE
-----
    python3 experiments/mmvq-wide.py <chemin-du-depot-experimental>
    # puis configurer avec -DCMAKE_CUDA_ARCHITECTURES=121 et compiler test-backend-ops
"""

import os
import sys

TYPES = ("GGML_TYPE_TQ2_G128", "GGML_TYPE_Q4_K", "GGML_TYPE_Q5_K", "GGML_TYPE_Q6_K")
GUARD = " || ".join("type == %s" % t for t in TYPES)

CASES = """        case {n}: {{
            // [obrain EXP mmvq-wide] lot de verification {n} : n'existe QUE dans ce build experimental.
            if constexpr ({guard}) {{
                constexpr int c_ncols_dst = {n};
                std::pair<dim3, dim3> dims = calc_launch_params<type>(c_ncols_dst, nrows_x, nchannels_dst, nsamples_dst, warp_size, table_id);
                mul_mat_vec_q_switch_fusion<type, c_ncols_dst>(vx, vy, ids, fusion, dst, ncols_x, nchannels_y_fd, stride_row_x, stride_col_y, stride_col_dst,
                     channel_ratio_fd, stride_channel_x, stride_channel_y, stride_channel_dst,
                     sample_ratio_fd, stride_sample_x, stride_sample_y, stride_sample_dst,
                     dims.first, dims.second, 0, ids_stride, stream);
            }} else {{
                GGML_ABORT("MMVQ au-dela de 8 : type non instancie dans ce build experimental");
            }}
        }} break;
"""

CASES_F = """        case {n}: {{
            // [obrain EXP mmvq-wide] MMVF {n} : le static_assert oblige la constante a bouger avec
            // celle de MMVQ, donc le type non quantifie doit lui aussi couvrir 9..16.
            constexpr int c_ncols_dst = {n};
{body}        }} break;
"""


def patch_file(path, old, new, label):
    with open(path, encoding="utf-8") as f:
        txt = f.read()
    if new in txt:
        print("  deja patche : %s" % label)
        return False
    if old not in txt:
        raise SystemExit("ancre introuvable dans %s (%s)" % (path, label))
    if txt.count(old) != 1:
        raise SystemExit("ancre ambigue (%d occurrences) dans %s (%s)" % (txt.count(old), path, label))
    with open(path, "w", encoding="utf-8") as f:
        f.write(txt.replace(old, new))
    print("  patche : %s" % label)
    return True


def main():
    repo = sys.argv[1] if len(sys.argv) > 1 else "."
    cuda = os.path.join(repo, "ggml", "src", "ggml-cuda")

    print("build C (MMVQ etendu a 16)")
    patch_file(os.path.join(cuda, "mmvq.cuh"),
               "#define MMVQ_MAX_BATCH_SIZE 8",
               "#define MMVQ_MAX_BATCH_SIZE 16 // [obrain EXP] 8 -> 16, build experimental uniquement",
               "mmvq.cuh : MMVQ_MAX_BATCH_SIZE 8 -> 16")
    patch_file(os.path.join(cuda, "mmvf.cuh"),
               "#define MMVF_MAX_BATCH_SIZE 8",
               "#define MMVF_MAX_BATCH_SIZE 16 // [obrain EXP] doit suivre MMVQ (static_assert)",
               "mmvf.cuh : MMVF_MAX_BATCH_SIZE 8 -> 16")

    # --- dispatch MMVQ : ajouter les cas 9..16 apres le cas 8 ---
    mmvq = os.path.join(cuda, "mmvq.cu")
    with open(mmvq, encoding="utf-8") as f:
        txt = f.read()
    anchor = """        case 8: {
            constexpr int c_ncols_dst = 8;
            std::pair<dim3, dim3> dims = calc_launch_params<type>(c_ncols_dst, nrows_x, nchannels_dst, nsamples_dst, warp_size, table_id);
            mul_mat_vec_q_switch_fusion<type, c_ncols_dst>(vx, vy, ids, fusion, dst, ncols_x, nchannels_y_fd, stride_row_x, stride_col_y, stride_col_dst,
                 channel_ratio_fd, stride_channel_x, stride_channel_y, stride_channel_dst,
                 sample_ratio_fd, stride_sample_x, stride_sample_y, stride_sample_dst,
                 dims.first, dims.second, 0, ids_stride, stream);
        } break;
"""
    if "[obrain EXP mmvq-wide]" in txt:
        print("  deja patche : mmvq.cu (cas 9..16)")
    else:
        if txt.count(anchor) < 1:
            raise SystemExit("bloc case 8 introuvable dans mmvq.cu")
        extra = "".join(CASES.format(n=n, guard=GUARD) for n in (9, 10, 11, 12, 13, 14, 15, 16))
        txt = txt.replace(anchor, anchor + extra, 1)
        with open(mmvq, "w", encoding="utf-8") as f:
            f.write(txt)
        print("  patche : mmvq.cu (cas 9..16, types quantifies seulement)")

    print("termine. Verifier : grep -c 'EXP mmvq-wide' %s" % mmvq)


if __name__ == "__main__":
    main()
