# Analyse hors ligne du CSV de caracterise_queue (#347) : python3 analyse_queue.py (lit queue.csv du repertoire courant).
import numpy as np, csv, sys
from collections import Counter
rows=list(csv.DictReader(open("queue.csv")))
def f(r,k):
    v=r[k]; return float(v) if v!="" else np.nan
A={k:np.array([f(r,k) for r in rows]) for k in ["rappel","rappel_l2","norme","cos_centroide","d1","d10","ratio","nb_doublons","longueur_texte","auto_trouve"]}
lab=np.array([r["label"] for r in rows])
n=len(rows); print("n",n)
for m in ["rappel","rappel_l2"]:
    x=np.sort(A[m]); print(m,"moy %.4f p05 %.2f p10 %.2f med %.2f"%(x.mean(),np.percentile(x,5),np.percentile(x,10),np.median(x)), "n<=0.3:",(x<=0.3).sum())
print("auto_trouve moyenne",np.nanmean(A["auto_trouve"]))
print("norme min/max",np.nanmin(A["norme"]),np.nanmax(A["norme"]))
print("longueur dispo",np.isfinite(A["longueur_texte"]).sum())
mal=A["rappel"]<=0.3; bien=~mal
print("mal servies",mal.sum(),"bien",bien.sum())
# bruit: d1 ~ d10 : ratio<1.1 ou d1==0 (doublons)
ratio=A["ratio"]; dup=A["nb_doublons"]>=1
for seuil in (1.05,1.1,1.25):
    bruit=(ratio<seuil)|(A["d1"]<=1e-6)
    print("bruit ratio<%.2f ou d1~0: %d ; parmi mal: %d/%d ; parmi bien: %d/%d"%(seuil,bruit.sum(),(bruit&mal).sum(),mal.sum(),(bruit&bien).sum(),bien.sum()))
print("avec doublon(s): total",dup.sum(),"mal",(dup&mal).sum())
print("d1~0 : total",(A["d1"]<=1e-6).sum(),"mal",((A["d1"]<=1e-6)&mal).sum())
def st(x):
    x=x[np.isfinite(x)]
    return (len(x),np.mean(x),np.percentile(x,25),np.median(x),np.percentile(x,75)) if len(x) else (0,)*5
print("%-15s %-6s %9s %9s %9s %9s | ecart moyennes"%("axe","groupe","n","moy","q1","med"),"q3")
for ax in ["norme","cos_centroide","d1","d10","ratio","longueur_texte","nb_doublons"]:
    for nm,g in (("mal",mal),("bien",bien)):
        s=st(A[ax][g]); print("%-15s %-6s %9d %9.4f %9.4f %9.4f %9.4f"%(ax,nm,*s[:4],s[4]))
    a=A[ax][mal];b=A[ax][bien]; a=a[np.isfinite(a)];b=b[np.isfinite(b)]
    if len(a) and len(b):
        # Mann-Whitney AUC
        allv=np.concatenate([a,b]); ranks=allv.argsort().argsort()+1.0
        # tie handling via scipy-free average ranks
        from itertools import groupby
        order=np.argsort(allv,kind="mergesort"); sv=allv[order]; rk=np.empty(len(allv)); i=0
        while i<len(sv):
            j=i
            while j+1<len(sv) and sv[j+1]==sv[i]: j+=1
            rk[order[i:j+1]]=(i+j)/2+1; i=j+1
        u=rk[:len(a)].sum()-len(a)*(len(a)+1)/2; auc=u/(len(a)*len(b))
        sd=np.sqrt(((a.var()*len(a)+b.var()*len(b))/(len(a)+len(b))))
        print("   -> AUC(mal>bien)=%.3f  d de Cohen=%.2f"%(auc,(a.mean()-b.mean())/sd if sd>0 else float("nan")))
# par quantile de d1 et ratio
for ax in ["d1","ratio","cos_centroide","longueur_texte"]:
    x=A[ax]; ok=np.isfinite(x); qs=np.percentile(x[ok],[0,20,40,60,80,100])
    print("rappel moyen par quintile de",ax)
    for i in range(5):
        m=ok&(x>=qs[i])&(x<=qs[i+1] if i==4 else x<qs[i+1])
        print("  [%.4f,%.4f] n=%d rappel=%.3f part<=0.3=%.3f"%(qs[i],qs[i+1],m.sum(),A["rappel"][m].mean(),(A["rappel"][m]<=0.3).mean()))
print("par label (n>=15):")
for l,c in Counter(lab).most_common():
    m=lab==l
    if c>=15: print("  %-20s n=%4d rappel=%.3f part<=0.3=%.3f dup=%.2f"%(l,c,A["rappel"][m].mean(),(A["rappel"][m]<=0.3).mean(),(dup[m]).mean()))
# mal servies hors bruit
bruit=(ratio<1.1)|(A["d1"]<=1e-6)
print("mal servies hors bruit(1.1):",(mal&~bruit).sum(),"; rappel moyen hors bruit",A["rappel"][~bruit].mean(),"p05",np.percentile(A["rappel"][~bruit],5), "n",(~bruit).sum())
print("mal hors bruit par label:",Counter(lab[mal&~bruit]).most_common(8))
print("rappel vs rappel_l2 sur mal: moy rappel_l2 sur mal",A["rappel_l2"][mal].mean())
