# `optimizers.lib` : vue d'ensemble

Version anglaise : [optimizers-overview-en.md](optimizers-overview-en.md)
(même contenu ; garder les deux versions synchronisées). Le complément
pratique est le tutoriel
[optimizers-ddsp-tutorial-fr.md](optimizers-ddsp-tutorial-fr.md).

Ce document s'adresse à quelqu'un qui connaît Faust mais n'a jamais rien
entraîné : aucune connaissance en apprentissage automatique, en
différenciation automatique ou en « DSP différentiable » n'est supposée. Il
explique à quoi sert la bibliothèque, pourquoi les primitives `fad`/`rad` sur
lesquelles elle repose méritent l'attention dans le domaine du DSP
différentiable, comment le fichier est organisé, et d'où vient chaque
algorithme et pourquoi il a été retenu. Chaque nombre cité a été mesuré sur le
compilateur courant avec `faustprobe` ; les programmes sont dans le tutoriel.

## 1. Trois idées

**Apprendre.** Un DSP a des paramètres : un gain, une fréquence de coupure,
cinq coefficients de biquad. D'ordinaire, un humain les règle. *Apprendre*,
c'est laisser le programme les régler lui-même, en comparant ce qu'il produit
à ce qu'il devrait produire et en déplaçant les paramètres dans la direction
qui réduit l'écart. Cet écart, ramené à un seul nombre, est la **perte** : par
exemple l'erreur quadratique `(modèle - cible)^2` à l'échantillon courant.

**Gradient.** Pour déplacer un paramètre dans la bonne direction, il faut
savoir comment la perte varie quand le paramètre varie : la dérivée de la
perte par rapport au paramètre. Pour plusieurs paramètres, le vecteur de ces
dérivées est le **gradient**. La *descente de gradient* est la mise à jour
`p <- p - lr * gradient`, répétée : `lr`, la vitesse d'apprentissage, fixe la
taille du pas. Tout ce que contient la bibliothèque est un raffinement de
cette ligne.

**Différenciation automatique.** Écrire les dérivées à la main est source
d'erreurs et ne survit pas à une modification du modèle. La différenciation
automatique (AD) les calcule mécaniquement à partir du programme qui calcule la
valeur, en appliquant la règle de dérivation en chaîne à chaque opération. Elle
est exacte (ce n'est pas une différence finie) et coûte un petit facteur
constant par rapport au calcul d'origine. L'apprentissage profond repose
dessus ; le *DSP différentiable* (DDSP) aussi : du traitement du signal dont les
paramètres sont entraînés par descente de gradient, introduit dans le monde
audio par Engel et al. en 2020.

## 2. Ce que `fad` et `rad` apportent de nouveau

### 2.1 Comment on fait du DDSP d'habitude

La pratique courante consiste à réécrire le DSP dans un framework
d'apprentissage (PyTorch, TensorFlow, JAX) : l'oscillateur, le filtre, la
réverbération deviennent des opérations sur tenseurs, le framework les
différencie, l'entraînement tourne hors ligne sur des lots d'audio en Python,
puis les paramètres appris sont exportés vers une implémentation temps réel
séparée. Cela marche, et c'est ainsi que la plupart des résultats DDSP publiés
ont été obtenus, mais avec des coûts que le praticien audio ressent
directement :

- le DSP existe deux fois, une pour l'entraînement et une pour le
  déploiement, et les deux doivent rester équivalents ;
- les filtres récursifs (IIR, rétroaction, tout ce qui contient un `~`) sont
  malcommodes : une boucle par échantillon est lente dans un framework à
  tenseurs, donc les filtres sont approchés, tronqués, ou dotés de noyaux
  dédiés ;
- l'apprentissage a lieu dans un script d'entraînement, pas dans l'instrument
  ou l'effet : un plugin adaptatif qui continue d'apprendre sur scène est hors
  de portée.

### 2.2 Ce que font les primitives Faust

`faust-rs` ajoute deux primitives au langage :

```faust
fad(expr, seeds)   // mode direct : sorties primales suivies de leurs tangentes
rad(expr, seeds)   // mode inverse : sorties primales suivies des gradients
```

La dérivée est calculée **par le compilateur, à la compilation, sur le graphe
de signaux lui-même**. `fad(expr, p)` n'est ni un graphe dynamique ni un appel
à un framework : il est développé pendant la propagation en signaux Faust
ordinaires, puis compilé en C++, Rust, WebAssembly ou pour l'interpréteur
comme n'importe quel autre signal. Trois conséquences comptent pour le DDSP :

1. **Un seul programme.** Le modèle, sa dérivée, la perte et l'optimiseur sont
   du code Faust dans le même fichier. Ce qui est entraîné est ce qui est
   déployé ; il n'y a ni étape d'export ni seconde implémentation.
2. **La récursion est différenciée exactement.** Une boucle de rétroaction est
   différenciée par *augmentation de son état* : la récursion transporte
   `[valeur, dérivée]` au lieu de `valeur` seule, si bien que la dérivée d'un
   filtre récursif par rapport à un coefficient est la dérivée causale exacte,
   échantillon par échantillon, sans fenêtre tronquée. En termes
   d'apprentissage automatique, c'est l'apprentissage récurrent en temps réel
   (RTRL, Williams & Zipser 1989), obtenu gratuitement de la structure du
   programme.
3. **L'apprentissage tourne là où tourne l'audio.** La mise à jour
   `p <- p - lr * g` est une récursion Faust comme une autre, donc un effet peut
   continuer à s'adapter dans un plugin, une page web ou une cible embarquée,
   un échantillon à la fois, avec les mêmes garanties temps réel que le reste
   du DSP.

L'idée a une lignée : les nombres duaux (Clifford, 1873), l'AD en mode direct
(Wengert, 1964), l'AD en mode inverse et la rétropropagation (Linnainmaa,
1976 ; Rumelhart et al., 1986). `faust-rs` l'introduit dans le langage avec
des graines explicites (`fad(expr, seeds)` : tout signal peut être une graine,
plusieurs à la fois), couvre la récursion, les tables et les blocs à domaine
d'horloge en mode direct, et implémente le mode inverse avec un balayage
arrière local au bloc pour les sorties remises à un hôte, et un balayage sur
un seul échantillon pour un gradient consommé dans le graphe. Les notes
techniques sont
[docs/fad-note-en.md](../docs/fad-note-en.md) et
[docs/rad-note-en.md](../docs/rad-note-en.md).

### 2.3 Direct ou inverse

| | `fad` (direct) | `rad` (inverse) |
|---|---|---|
| Sortie | chaque primale suivie d'une tangente par graine | toutes les primales, puis un gradient par graine |
| Le coût croît avec | le nombre de graines (paramètres) | le nombre de sorties : un balayage donne tous les gradients |
| À travers la récursion | dérivée causale exacte (état augmenté) | en sortie publique : balayage local au bloc `compute` courant, adjoint terminal nul ; consommé dans le graphe : l'échantillon lui-même, l'état passé tenu fixe (le terme direct) |
| Consommable dans le graphe | oui, le choix naturel pour un optimiseur dans le graphe | oui : le même gradient pour un modèle sans récursion, le gradient de la régression pseudo-linéaire à travers une récursion |
| Usage typique | boucles d'apprentissage en Faust, solveurs de Newton, pentes locales | beaucoup de paramètres sous une seule perte (les boucles à bus), gradients remis à un hôte qui accumule sur un bloc |

Pour une poignée de paramètres interprétables — ce qu'a d'ordinaire un modèle
audio — le mode direct est le bon outil pour une boucle écrite en Faust, et
c'est ce qu'utilisent les boucles à arité fixe d'`optimizers.lib`. Le mode
inverse est la direction économique quand une perte scalaire dépend de
nombreux paramètres : un balayage les donne tous, là où le mode direct
transporte une tangente par paramètre. Les boucles à bus de la bibliothèque
existent dans les deux modes ; sur un FIR à 16 coefficients, la version `rad`
compile en 3× moins d'instructions d'interpréteur et tourne 2,5× plus vite,
7× et 10× à 64 coefficients, pour la même trajectoire. Le prix, dans une
boucle, c'est l'horizon : le gradient est consommé à l'échantillon qui le
produit, donc le balayage inverse ne voit que cet échantillon, et à travers
une récursion du modèle il renvoie le terme direct — l'état passé tenu fixe —
là où `fad` transporte la dérivée exacte (section 4.7). Pour un modèle sans
récursion (un FIR, un gain, un waveshaper), les deux sont le même nombre.
Remis à un hôte en sortie publique, `rad` travaille bloc par bloc ; voir
[docs/rad-usage-en.md](../docs/rad-usage-en.md).

### 2.4 Domaines d'horloge : `ondemand` et l'apprentissage à sa propre cadence

Faust calcule chaque signal une fois par échantillon. `faust-rs` ajoute trois
primitives qui permettent à une sous-expression de tourner **à sa propre
cadence** : `ondemand(C)`, `upsampling(C)` et `downsampling(C)`. Chacune prend
un corps `C` et le renvoie avec une entrée de plus, une *horloge* :
`ondemand(C)` n'exécute `C` que sur les échantillons où l'horloge est non nulle
et **maintient** ses sorties entre deux ; dans le corps, le temps est le *temps
de tir* — une récursion `~` ou un délai compte les tirs, pas les échantillons
audio. La note [docs/ondemand-note-fr.md](../docs/ondemand-note-fr.md) est la
référence ; ce qui compte ici est le partenariat avec `fad`.

Une boucle d'apprentissage contient deux cadences que rien n'oblige à être
égales : la cadence audio, à laquelle le modèle doit tourner, et la cadence
d'*adaptation*, à laquelle les paramètres bougent. Les frameworks
d'entraînement déplacent les paramètres une fois par lot ; les filtres
adaptatifs une fois par échantillon ; un bloc `ondemand` laisse le programme
Faust choisir, et la machinerie de dérivation se compose avec lui :

- **`fad` dans un bloc** est pris en charge et vérifié contre des différences
  finies par les tests du compilateur : la primale et ses tangentes sont
  calculées ensemble à chaque tir et maintenues ensemble entre deux. Les
  entrées du bloc (une trame de `N` échantillons, par exemple) sont des
  signaux à tangente nulle ; une graine qui vit dans le corps, ou qui y entre
  comme entrée explicite, est différenciée normalement.
- **`fad` autour d'un bloc** différencie à travers la sortie maintenue.
- **Une dérivée ne traverse jamais seule une frontière d'horloge.** L'horloge
  est opaque à la différenciation, `rad` à travers une frontière de domaine est
  refusé, et la règle de la note `ondemand` est celle à suivre : garder la
  graine, la perte et la mise à jour dans le même domaine, et passer au bloc
  tout ce dont le corps a besoin de l'extérieur comme entrée *explicite*.

Trois motifs en découlent, tous mesurés (les programmes sont à la section 11
du tutoriel) :

| Motif | Où tournent les choses | Résultat |
|---|---|---|
| **Apprentissage à cadence de contrôle** : toute la boucle `descend_1D` dans `ondemand`, cadencée tous les 64 échantillons | modèle, perte, `fad` et mise à jour en temps de tir ; le paramètre est maintenu entre deux tirs | gain 0 → 0,7000 en 312 pas d'optimiseur sur 20 000 échantillons ; le graphe `fad` tourne 64 fois moins souvent |
| **Gradient à cadence audio, mise à jour par trame** : `fad` et une `ema` du gradient à cadence audio, le pas de paramètre dans un bloc qui reçoit la valeur précédente et le gradient moyenné en entrées, le paramètre maintenu rebouclé par `~` | gradient à cadence audio, mise à jour par trame | gain exact (`0,700000`) après 4 000 échantillons : moyenner le gradient sur la trame ne perd rien |
| **Perte spectrale par trame** : `interleave.lib` sérialise l'audio en trames de `N` échantillons, et `descend_1D` tourne dans le bloc sur une perte calculée par `an.fft` sur la trame — un pas d'optimiseur par trame | tout dans le domaine de la trame ; c'est la perte spectrale du DDSP, en Faust | un gain qui met à l'échelle un spectre à 8 points se stabilise à 0,34, l'optimum des moindres carrés pour cette excitation valant 0,340 |

Le troisième motif est celui qui rapproche le plus Faust de la façon dont les
articles DDSP entraînent : une perte sur un spectre plutôt que sur une forme
d'onde, mise à jour par trame. Sa variante avec le paramètre gardé hors du bloc
et passé en entrée explicite (le bloc sort alors le gradient maintenu et la
mise à jour est conditionnée par l'horloge) donne le même 0,34.

Le deuxième motif est ce que la bibliothèque empaquette sous les noms
`descend_1D_clocked` … `descend_5D_clocked` : la perte et `fad` à cadence
audio, le gradient moyenné sur la trame par `frame_mean` (une moyenne exacte,
remise à zéro par l'horloge), le pas pris dans un bloc `ondemand` pour que les
moments et les schedules du moteur avancent une fois par trame, le paramètre
maintenu à `init` jusqu'au premier tir et un reset plus court qu'une trame
verrouillé jusqu'au tir suivant. Mesuré : un gain exact (`0,700000`) après
4 000 échantillons avec un pas SGD de 0,5 par trame de 64 échantillons ;
`(log f, q)` d'un filtre résonant avec Adam par trame à `(1200,2, 1,996)` après
10 000 échantillons, puis à moins de 1 % de `(1200, 2,0)`.

Deux règles pratiques. Un opérateur de trame écrit avec des
entrées `_` libres ne doit pas circuler comme expression ouverte — chaque usage
duplique ses entrées et l'arité du bloc explose ; donner au corps des arguments
nommés (`learn(x0, ..., x7)`). Et `ma.SR` n'est *pas* adapté dans `ondemand` (sa
cadence n'est pas connue statiquement) : tout ce qui dépend de la cadence
propre du bloc doit être calculé à l'extérieur et passé en entrée.

### 2.5 Ce que ce n'est pas

`fad`/`rad` ne font pas de Faust un framework d'apprentissage profond, et cette
bibliothèque ne prétend pas le contraire :

- les graines sont explicites : on nomme ce par rapport à quoi on dérive ;
- certains nœuds n'ont pas de règle de dérivation et donnent silencieusement
  une **tangente nulle** : boutons, cases à cocher, arithmétique et
  comparaisons entières, conversions en entier, écritures de tables. Un
  paramètre appris ne doit pas les traverser dans le modèle ;
- `abs` est dérivé comme `x / abs(x)`, qui vaut `NaN` en zéro — les pertes de
  la bibliothèque l'évitent ;
- il n'y a ni lots ni chargeur de données : l'apprentissage est en ligne, un
  échantillon à la fois, sur l'audio qui traverse le programme.

## 3. Organisation de la bibliothèque

Le fichier [optimizers.lib](optimizers.lib) (préfixe `op`, version 0.7.2) est
documenté fonction par fonction selon la convention des bibliothèques Faust ;
cette section en donne la carte. Il comporte douze sections, ordonnées des
briques de base aux boucles prêtes à l'emploi.

| Section | Contenu | Raison d'être |
|---|---|---|
| Signal helpers and parameter state | `clip`, `sgn`, `ema`, `ema_bc`, `pstate`, `polyak` | les quelques primitives avec lesquelles tout moteur et toute boucle sont écrits, par-dessus `si`, `ba`, `ro`, `ma` |
| Losses and regularizers | `mse`, `pseudo_huber`, `logcosh`, `energy_loss`, `log_energy_loss`, `l2`, `l1s` | une perte est une fonction Faust ordinaire `loss(y, t)` ; celles-ci sont lisses |
| Reparameterizations | `poles_from_reflection`, `reflection_from_poles`, `sigmoid_map` | apprendre dans un domaine où toute valeur est admissible (stable, positive, bornée) plutôt que borner |
| Gradient conditioning and schedules | `clip_g`, `softclip_g`, `gate_g`, `lr_exp`, `lr_cos`, `warmup` | ce qui arrive à un gradient avant le moteur, et comment une vitesse d'apprentissage évolue |
| Least-squares engines | `lms`, `nlms`, `gn1`, `sgd`, `adam`, `rmsprop`, `nadam`, `sign_sgd` | moteurs qui voient séparément le résidu `r` et la sensibilité `j` |
| Gradient engines | `sgd_g`, `momentum_g`, `nesterov_g`, `adam_g`, `nadam_g`, `amsgrad_g`, `adabelief_g`, `rmsprop_g`, `adagrad_g`, `lion_g`, `sign_g` | moteurs qui voient un seul nombre, le gradient de la perte `g` |
| Least-squares loops | `lsq_1D` … `lsq_5D`, `optimize_1D` … `optimize_5D` | le modèle est différencié, la perte est implicitement l'erreur quadratique |
| Loss-first loops | `descend_1D` … `descend_5D` | la perte est différenciée, quelle qu'elle soit |
| Gauss-Newton loops | `lm_2D`, `lm_3D` | pas de second ordre pour deux ou trois paramètres corrélés |
| Bus loops | `lsq_N`, `descend_N`, `descend_N_clocked` et `lsq_N_rad`, `descend_N_rad`, `descend_N_rad_clocked` | `N` paramètres en bus avec un moteur et une paire de bornes, en mode direct ou inverse |
| Clocked loops | `frame_sum`, `frame_count`, `frame_mean`, `descend_1D_clocked` … `descend_5D_clocked` | le gradient à cadence audio, moyenné sur la trame, le pas une fois par tir d'une horloge `ondemand` |
| Newton solver | `newton_step`, `newton` | pas de l'apprentissage : résoudre une équation implicite avec `F` et `F'` issus d'un seul `fad` |

### 3.1 La forme d'une boucle

Toute boucle est la même récursion, dessinée ici pour un paramètre :

```text
              prev (état récursif)
                │
        init + écart                 la récursion garde l'écart à init ; le pas s'y applique ; reset l'efface
                │
          clip(lo, hi, ·)            projection sur les bornes
                │
                p ──────────────────────────────┐
                │                               │
      fad(loss(p), p) : !, _      ou      fad(mdl(p, x), p) : (r, j)
                │                               │
            moteur(g)                       moteur(r, j)
                │                               │
          clip(lo, hi, p - pas)  ───────────────┘
                │
              next  ──►  mémorisé pour l'échantillon suivant
```

Le paramètre vit dans l'état récursif de Faust sous la forme de son écart à la
valeur initiale : la récursion part de zéro, `reset` l'efface, et aucune
détection du premier échantillon n'intervient, si bien que la boucle marche
aussi dans un bloc `ondemand` quel que soit son premier tir. Le pas s'applique
à l'écart et le bornage se fait dans l'espace de l'écart, si bien qu'un pas
plus petit que la précision de la valeur n'est pas perdu (un paramètre proche
de 1000 appris en simple précision garde des pas de 1e-5) ; un appel `fad` par
échantillon fournit la dérivée ; le moteur
transforme la dérivée en pas. Avec `N` paramètres, un seul appel `fad` à `N`
graines produit les `N` dérivées d'un coup. Les boucles à bus dessinent la
même figure avec `N` fils au lieu d'un, et `rad` à la place de `fad` dans
leurs versions `_rad`.

### 3.2 Deux familles, et pourquoi

La famille **moindres carrés** (`lsq_ND`, `lm_ND`) différencie le *modèle* et
remet à chaque moteur deux nombres : le résidu `r = modèle - cible` et la
sensibilité `j = d(modèle)/dp`. La perte est implicitement `r^2`, mais garder
`r` et `j` séparés est ce qui rend la normalisation possible — NLMS divise le
pas par la puissance de `j`, Gauss-Newton résout les équations normales
construites à partir des `j` — et la normalisation est l'astuce la plus utile
du filtrage adaptatif.

La famille **perte d'abord** (`descend_ND`) différencie une perte scalaire que
l'utilisateur écrit comme une fonction Faust ordinaire fermée sur les données,
et remet à chaque moteur un seul nombre, `g = d(perte)/dp`. Tout ce qui est
différentiable est une perte valide : une perte robuste, une comparaison
d'énergie qui ignore la phase, un modèle à plusieurs sorties réduit à un
scalaire, une pénalité sur les paramètres ajoutée à l'erreur. Le prix : le
moteur ne voit plus `r` et `j` séparément et ne peut donc pas normaliser par la
sensibilité ; les moteurs adaptatifs (Adam, Lion) jouent ce rôle.

Les **boucles à bus** (`lsq_N`, `descend_N`, `descend_N_clocked`) sont les
deux mêmes familles pour `N` paramètres portés par un bus, `N` constant, avec
un moteur et une paire de bornes pour tous — la forme d'un FIR adaptatif ou
d'une rangée de gains — là où les boucles à arité fixe donnent à chaque
paramètre les siens. Chacune a une jumelle `_rad` : un balayage inverse par
échantillon pour les `N` dérivées au lieu de `N` tangentes. La section 4.7
dit ce que ce balayage calcule à travers une récursion.

Les points d'entrée d'origine `optimize_ND` sont conservés comme enveloppes de
`lsq_ND` (valeur initiale nulle, pas de reset).

### 3.3 Le contrat des moteurs

Un moteur est une fonction dont le ou les deux derniers arguments sont
l'information de dérivée ; tout ce qui précède est réglage. L'application
partielle produit le reste à un ou deux arguments qu'attend une boucle :

```faust
op.descend_1D(loss, op.adam_g(0.01, 0.9, 0.999, 1e-8), lo, hi, init, reset);
op.lsq_3D(fir, op.nlms(0.02, 1e-6, 0.99), op.nlms(0.02, 1e-6, 0.99), op.nlms(0.02, 1e-6, 0.99), ...);
```

Chaque application est une instance distincte avec son propre état : deux
paramètres partageant la même expression de moteur ne partagent pas leurs
moments. Toute vitesse d'apprentissage est un signal, c'est pourquoi un
schedule tel que `op.lr_exp(...)` se passe simplement à la place d'une
constante.

### 3.4 Bibliothèques standard

La bibliothèque importe `signals.lib`, `basics.lib`, `routes.lib` et
`maths.lib` (`si.smooth`, `si.bus`, `ba.time`, `ro.interleave`, `ma.PI`) : le
répertoire des bibliothèques standard de Faust doit donc être sur le chemin
d'import, à côté de `libraries`. La suite de tests de `faust-rs` le trouve par
`FAUST_RS_FAUSTLIBRARIES_ROOT` ou par un chemin par défaut, et saute les tests
de la bibliothèque quand ni l'un ni l'autre n'existe, si bien que la suite
reste exécutable sans distribution Faust. Onze fixtures dans
`tests/corpus/opt_*.dsp` passent par l'interpréteur en CI, dont un généré à
partir de l'entrée `#### Test` de chaque fonction documentée, si bien que les
exemples de la documentation sont compilés eux aussi.

## 4. D'où viennent les algorithmes, et pourquoi ceux-là

### 4.1 Moteurs de mise à jour

| Moteur | Origine | Pourquoi il est là |
|---|---|---|
| `lms` | Widrow & Hoff, 1960 — le filtre adaptatif LMS | l'ancêtre de tout le reste ; une multiplication |
| `nlms` | Nagumo & Noda, 1967 — LMS normalisé | les niveaux audio varient de 40 dB ; diviser le pas par la puissance de la sensibilité rend la convergence indépendante du niveau. Mesuré sur un FIR à 3 coefficients : LMS est 100× trop lent au niveau 0,1 et sature au niveau 10, NLMS converge à l'identique à 0,1, 1 et 10 |
| `gn1` | Gauss-Newton / moindres carrés récursifs | la version à un paramètre de `lm_2D` |
| `sgd_g`, `momentum_g`, `nesterov_g` | Robbins & Monro 1951 ; Polyak 1964 ; Nesterov 1983 | les pas standard de l'apprentissage automatique ; le moment moyenne les gradients bruités |
| `adagrad_g` | Duchi et al., 2011 | un pas décroissant (la somme des carrés ne fait que croître) ; utile quand un paramètre doit se figer pour de bon |
| `rmsprop_g` | Tieleman & Hinton, 2012 | normalisation par l'amplitude récente du gradient |
| `adam_g`, `nadam_g` | Kingma & Ba, 2015 ; Dozat, 2016 | le défaut de l'apprentissage profond : moment plus normalisation par paramètre. La correction de biais est implémentée comme `ema(a, g) / ema(a, 1)`, puisque `1 : smooth(a)` vaut exactement `1 - a^(n+1)` ; sans elle, les premiers pas valent `3,16 * lr` |
| `amsgrad_g`, `adabelief_g` | Reddi et al., 2018 ; Zhuang et al., 2020 | variantes d'Adam qui n'augmentent jamais le pas effectif (AMSGrad) ou normalisent par la variance du gradient plutôt que par son amplitude (AdaBelief) |
| `lion_g` | Chen et al., 2023 | des pas de `±lr` dans la direction du signe d'un moment : une seule vitesse pour des paramètres de toute unité, une seule variable d'état. Mesuré : cinq coefficients de biquad appris avec une seule vitesse Lion, tous à moins de 3e-6 de la cible |
| `sign_g`, `sign_sgd` | descente par le signe | le pas insensible à l'échelle le plus simple |

La bibliothèque conserve les `sgd`, `adam`, `rmsprop`, `nadam`, `sign_sgd`
d'origine avec leurs signatures ; `adam` et `nadam` ont gagné la correction de
biais.

Pourquoi tant de moteurs ? Parce que les paramètres d'un modèle audio ont des
unités très différentes — une fréquence en hertz, un facteur de qualité, un
coefficient de filtre dans `(-1, 1)` — et que la descente de gradient nue
demande une vitesse par unité. Les moteurs adaptatifs normalisent le pas de
chaque paramètre par son propre historique de gradient, ce qui permet à un seul
`lr` de servir tout un modèle. C'est la réponse pragmatique ; la réponse de
principe est la famille suivante.

### 4.2 Second ordre : `lm_2D`, `lm_3D`

Gauss-Newton (Gauss, 1809, pour des orbites) utilise les sensibilités `j` pour
construire les *équations normales* et les résout, ce qui met chaque paramètre
à l'échelle de sa propre courbure et tient compte des corrélations entre
paramètres. Levenberg (1944) et Marquardt (1963) ont ajouté un amortissement
pour que le pas reste raisonnable loin de la solution ; Ljung & Söderström
(1983) ont donné la forme récursive à facteur d'oubli utilisée en
identification de systèmes (méthodes récursives d'erreur de prédiction), dont
les moindres carrés récursifs sont le cas linéaire.

`lm_2D` implémente cette forme récursive : la matrice d'information est
moyennée avec un facteur d'oubli (avec correction de biais), amortie par
`lambda * diag(H)` pour que l'amortissement soit sans unité, et appliquée à
l'innovation *instantanée* `j * r`. Mesuré sur `fi.resonlp(f, q)` :
`(1200,000000, 2,000000)` depuis `(1000, 1)` en 5 000 échantillons avec un
seul gain, là où le RMSProp par paramètre de la note de synthèse demandait
`lr_f = 2,0` et `lr_q = 0,01`. Il est limité à deux et trois paramètres parce
que Faust n'a pas de matrices ; pour les modèles interprétables que vise cette
bibliothèque, c'est en général suffisant. Notez qu'appliquer le pas à
l'innovation *moyennée* plutôt qu'instantanée reviendrait à appliquer la même
correction une fois par échantillon sur toute la fenêtre, et diverge.

### 4.3 Pertes

| Perte | Origine | Pourquoi |
|---|---|---|
| `mse` | moindres carrés | le défaut ; son gradient est `2 r` |
| `pseudo_huber`, `logcosh` | Huber, 1964 ; régression log-cosh | quadratiques près de zéro, linéaires au loin, *lisses* partout : une valeur aberrante dans la cible déplace le paramètre d'au plus `lr`. Mesuré avec des impulsions de `±20` tous les 97 échantillons sur un signal d'amplitude 0,7 : `mse` garde une gigue de 0,2 sur le gain, `logcosh` 0,007, `pseudo_huber` 0,0003 |
| `energy_loss`, `log_energy_loss` | appariement de puissance ; le cousin par échantillon des pertes spectrales du DDSP | comparer des puissances lissées ignore la phase, donc un modèle peut être ajusté à une cible excitée par une *autre* réalisation de l'excitation, là où l'erreur échantillon par échantillon n'a pas de sens. La version log est sans échelle |
| `l2`, `l1s` | Tikhonov / ridge ; lasso, lissé | la régularisation comme terme de perte : `loss(p) = mse(...) + l2(0.001, p)` |

La lissité est le critère de sélection : `abs` et `max(0, ·)` n'apparaissent
dans aucune perte, parce que la dérivée de `abs` n'est pas définie en zéro et
que le compilateur ne la régularise pas.

### 4.4 Reparamétrisations

Un coefficient appris d'un filtre récursif doit rester dans la région où le
filtre est stable. Borner `a1` et `a2` d'un biquad à un rectangle n'y suffit
pas : la région de stabilité est un triangle (`|a2| < 1`, `|a1| < 1 + a2`), et
le rectangle utilisé par l'exemple d'origine admet des points instables. Parti
de `(a1, a2) = (1,9, -0,5)` — dans le rectangle, hors du triangle — le modèle
diverge vers `inf` et la projection n'y peut plus rien. `poles_from_reflection`
apprend à la place deux *coefficients de réflexion* `k1, k2` dans `(-1, 1)` et
les transforme par `a1 = k1 (1 + k2)`, `a2 = k2` : c'est la paramétrisation en
treillis d'Itakura & Saito et de   Gray (1976), une bijection sur le
triangle, si bien qu'une boîte rectangulaire sur `(k1, k2)` ne contient que des
filtres stables. `sigmoid_map` remplace les bornes dures par une sigmoïde
logistique, comme le font les articles DDSP pour contraindre leurs paramètres.
La recette « fréquence en log » (`mdl(exp(u), x)` avec bornes
`log(lo), log(hi)`) en est le pendant audio : un pas sur `u` est une variation
relative de fréquence, ce que l'oreille et le gradient réclament tous deux.

### 4.5 Schedules, gating, lecture

Les schedules de vitesse d'apprentissage (décroissance exponentielle ;
recuit en cosinus, Loshchilov & Hutter 2017 ; warm-up) concilient un départ
rapide et une fin calme — en audio, la « fin calme » est l'absence de gigue
audible sur un paramètre. `gate_g` n'apprend que lorsqu'une condition est
vraie, typiquement quand il y a du signal : le même gating qu'emploient les
filtres adaptatifs pour ne pas dériver dans le silence. `polyak` (Polyak &
Juditsky, 1992) moyenne le paramètre pour la lecture audible pendant que
l'optimiseur continue d'avancer sur la valeur brute.

### 4.6 Newton

`newton` n'est pas un optimiseur : il résout `F(y) = 0` par Newton-Raphson,
avec `F` et `F'` produits par un seul appel `fad`. Il est là parce que c'est la
même primitive mise à un autre usage, et parce que les équations implicites
sont partout dans la modélisation analogique virtuelle (filtres à rétroaction
sans délai, écrêteurs à diodes : Zavalishin, *The Art of VA Filter Design*).

### 4.7 Le mode inverse dans une boucle : la régression pseudo-linéaire

Un gradient consommé à l'échantillon qui le produit ne peut pas attendre la
fin du bloc : le balayage inverse des boucles `_rad` ne voit qu'un
échantillon. L'adjoint remonte les opérations du modèle de cet échantillon et
s'arrête à son état récursif, tenu fixe. Pour un modèle récursif
`y[n] = x[n] + p y[n-1]`, cela donne `d(perte)/dp = 2 r y[n-1]`, le *terme
direct* ; `fad` donne `2 r dy[n]/dp` avec `dy[n]/dp = y[n-1] + p dy[n-1]/dp`,
la dérivée à travers la récursion. En filtrage adaptatif, le terme direct est
le gradient de la **régression pseudo-linéaire** (le LMS récursif de
Feintuch, 1976 ; Shynk 1989) et le gradient récursif celui de l'*erreur de
prédiction récursive* (Ljung & Söderström 1983) : le premier est moins cher
et converge vers la même solution sous une condition de positivité sur le
modèle, le second est la direction de descente exacte. La bibliothèque offre
les deux — `fad` dans les boucles à arité fixe et dans `lsq_N`/`descend_N`,
le terme direct dans les jumelles `_rad` — et pour un modèle sans récursion il
n'y a aucune différence, ce qui est précisément là où le mode inverse paie :
beaucoup de paramètres, un seul balayage.

### 4.8 Ce qui a été laissé de côté, et pourquoi

- **RLS / Gauss-Newton au-delà de trois paramètres** : Faust n'a pas de
  matrices ; `lm_2D`/`lm_3D` couvrent les modèles interprétables que vise la
  bibliothèque, et les jeux de paramètres plus grands sont mieux servis par
  les moteurs adaptatifs du premier ordre.
- **AdamW** (décroissance de poids découplée) : ici, un régulariseur est un
  terme de perte, `l2(lambda, p)`, qui se compose avec tous les moteurs.
- **L-BFGS et autres méthodes par lots** : elles demandent un lot et une
  recherche linéaire ; en ligne, un échantillon à la fois, elles n'ont pas de
  forme naturelle.
- **Pertes spectrales** (STFT multi-échelles) : elles demandent une trame, ce
  qui en Faust signifie un bloc `ondemand` ; le fixture
  `tests/corpus/ondemand_fad_spectral_loss_008.dsp` montre `fad` à travers une
  perte basée sur une FFT. L'intégrer à la bibliothèque est un travail à venir.

## 5. Comportement mesuré

Toutes les exécutions : `faustprobe --double -I libraries -I <faustlibraries>` ;
programmes dans le tutoriel.

| Expérience | Résultat |
|---|---|
| Gradient écrit à la main vs `fad`, gain appris dans une récursion | identiques (différence `0`) |
| `descend_1D` + `adam_g(0.002)`, gain 0 → 0,7 | 0,6998 à 1 500 échantillons, exact à 2 500 |
| `lsq_1D` + `nlms`, un pôle `p* = 0,6` | 0,600013 à 500, exact à 2 000 |
| `lsq_3D` + `nlms` vs `lms`, FIR à 3 coefficients, niveau d'entrée 0,1 / 1 / 10 | NLMS exact aux trois niveaux ; LMS 100× trop lent à 0,1, saturé à 10 |
| Lion, un seul `lr = 0,001`, sur `(f, q)` en hertz et en Q | `q` converge, `f` avance de 1 Hz par 1 000 échantillons : le problème d'échelle |
| Lion sur `(log f, q)` | `(1206, 2,003)` à 10 000 échantillons, puis gigue d'environ 2 % |
| `lm_2D` sur `(f, q)` | `(1200,000000, 2,000000)` à 5 000 échantillons |
| Biquad à cinq coefficients, bornes rectangulaires sur `(a1, a2)`, départ `(1,9, -0,5)` | diverge (`inf`) |
| Idem avec coefficients de réflexion, `descend_5D` + Lion | les cinq à moins de 1e-5 de la cible à 300 000 échantillons |
| `logcosh` vs `mse` sous impulsions de `±20` | gain dans 0,69–0,71 contre 0,49–0,88 |
| `log_energy_loss`, réalisations de bruit indépendantes, `fc* = 800` | 770–850 Hz (`mse` reste sur la borne 20 Hz) |
| `newton(5)` sur `y = tanh(x - 2y)` | résidu `0` à chaque trame |
| `descend_1D` dans `ondemand`, un pas tous les 64 échantillons | gain 0 → 0,7000 en 312 pas (20 000 échantillons) |
| `fad` + `ema` à cadence audio, mise à jour dans un bloc de 64 échantillons | gain `0,700000` à 4 000 échantillons |
| `descend_1D` sur une perte FFT à 8 points dans le bloc de trame | gain 0,34 (optimum des moindres carrés 0,340) |
| `descend_1D_clocked`, SGD 0,5 par trame de 64 échantillons | gain `0,700000` à 4 000 échantillons |
| `descend_2D_clocked`, Adam par trame sur `(log f, q)` | `(1200,2, 1,996)` à 10 000 échantillons, puis à moins de 1 % |
| `lsq_N_rad` + `nlms`, FIR à 8 coefficients au niveau 10 | résidu sous 1e-6 à partir de 1 000 échantillons |
| `descend_N` contre `descend_N_rad`, FIR à 16 coefficients, LMS 0,02 | même résidu à l'arrondi près ; 3 777 contre 1 182 instructions d'interpréteur, 0,10 s contre 0,04 s pour 200 000 échantillons ; 28 891 contre 4 129 et 1,32 s contre 0,13 s à 64 coefficients |
| `rad` contre `fad` dans le graphe sur `y = 1 + p y[n-1]`, `perte = (y - 3)^2` | `rad` -3, -3,75, -3,94 (terme direct), `fad` -3, -5, -6,19 (à travers la récursion) |

## 6. Pièges à connaître

- **La graine doit être le nœud utilisé dans le modèle.** Les graines sont
  reconnues par identité après abaissement, pas par équivalence algébrique.
  Dans une boucle, `fad(loss(p), p)` avec `p` l'entrée récursive est la
  dérivée partielle exacte.
- **Une graine est reconnue par identité, et chaque occurrence compte.**
  `fad(F(v, v), v)` dérive les deux occurrences de `v` : l'itération de
  Newton d'un solveur implicite ne doit pas partir du signal même que
  l'équation tient fixe (`vprev`) — partir d'un prédicteur, ou de tout
  signal distinct. Inversement un signal que la graine n'atteint pas (la
  sortie d'une autre boucle, un générateur de bruit) garde une tangente
  nulle et n'est pas réécrit.
- **Convention de signe.** Avec `r = modèle - cible`, le gradient MSE est
  `+2 r j`. La note de synthèse écrit `err = cible - modèle` et `-err * j`. Les
  deux sont justes ; les mélanger fait remonter la perte.
- **Les pertes lissées ont un retard.** `energy_loss` regarde une puissance
  moyennée sur `1 / (1 - a)` échantillons ; la constante de temps propre de
  l'optimiseur doit être plus longue, sinon la boucle à retard oscille.
- **Adam et pertes bruitées.** Adam normalise le pas : sur une perte bruitée,
  le paramètre fait une marche aléatoire de `lr` par échantillon ; le SGD nu,
  dont le pas suit l'amplitude du gradient, s'éteint de lui-même. Schedules et
  `polyak` sont le remède quand on tient à Adam.
- **Faust n'a pas de destructuration.** `(a1, a2) = poles_from_reflection(k1, k2)`
  ne parse pas ; écrire `a1 = ... : _, !;`. Et appliquer une expression à cinq
  sorties à une fonction à cinq arguments est une application partielle, pas un
  étalement : projeter chaque sortie.
- **Domaines d'horloge.** Dans un bloc `ondemand`, une récursion avance une
  fois par tir et `ma.SR` n'est pas adapté ; un corps doit recevoir les signaux
  extérieurs comme entrées explicites, et un opérateur de trame à entrées `_`
  libres doit recevoir des arguments nommés, sinon ses entrées sont dupliquées à
  chaque usage.
- **Double précision.** Les gradients des filtres récursifs perdent vite en
  précision en simple précision ; compiler les programmes d'apprentissage avec
  `-double`.
- **`op.mse(_, cible)` a deux entrées.** Un `_` libre est dupliqué partout où
  l'argument est utilisé : `(_ - t) * (_ - t)` est un bloc à deux entrées, et
  un `:>` vers lui répartit un bus entre elles — les coefficients font une
  marche aléatoire autour de zéro. Nommer l'entrée : `\(y).(op.mse(y, cible))`.
- **`rad` dans une boucle ne voit qu'un échantillon.** À travers une récursion
  il renvoie le terme direct, pas la dérivée à travers la récursion (section
  4.7) ; apprendre les modèles récursifs avec les boucles `fad`, les modèles
  sans récursion avec les unes ou les autres.

## 7. Portée : ce que cela atteint, et ce que cela n'atteint pas

Face aux frameworks à tenseurs de la DDSP (PyTorch ou JAX avec les
bibliothèques DDSP, torchaudio, FLAMO, dasp-pytorch), la différentiation au
niveau du compilateur est à la DDSP ce que le filtrage adaptatif est à
l'apprentissage automatique : exacte, bon marché, temps réel, interprétable,
petite. Son domaine, ce sont les modèles paramétriques dont un ingénieur du
son sait lire les paramètres. La calibration d'un réseau de lignes à retard
sur des salles mesurées, hors ligne et dans le processus audio, en est
l'exemple travaillé (le projet `faust-diff-fdn`) ; cette section est ce
qu'il a appris sur la portée de l'approche.

**Ce qu'on peut raisonnablement atteindre.**

- *Calibration et identification de systèmes* : réverbérations, filtres,
  égaliseurs, modèles physiques, circuits à boîte grise, avec des dizaines à
  quelques centaines de paramètres. `rad` coûte environ trois passes avant
  quel que soit leur nombre, donc quelques centaines restent abordables hors
  ligne.
- *L'apprentissage dans le processus audio*, ce qu'aucun framework ne fait :
  effets qui se calibrent, suivi d'une cible qui dérive, annulation d'écho,
  filtres adaptatifs, patches qui s'accordent, et l'apprentissage coupé par
  une horloge `ondemand` une fois fini, sans coût ensuite. Réaliste jusqu'à
  quelques dizaines de paramètres en temps réel avec `fad`, sur des cibles
  embarquées ou dans un navigateur, puisque le programme qui apprend est du
  Faust ordinaire.
- *Les petits réseaux écrits en Faust* : un GRU d'ampli, un MLP de quelques
  centaines de poids (exemples 5 et 9 de `ddsp-examples-fr.md`).
  Entraînables, mais lentement : sur CPU, un exemple à la fois.
- *La conception par objectif* : les paramètres d'une structure fixe qui
  atteignent une spécification là où les formules analytiques n'existent
  pas ; et les solveurs de Newton pour les circuits implicites, déjà en
  place.
- *Le pont avec les frameworks*, à portée mais pas fait : un programme Faust
  comme couche différentiable dans PyTorch. `rad` produit les produits
  vecteur-jacobien qu'une `autograd.Function` attend, et dans l'autre sens un
  encodeur entraîné en PyTorch s'exporte vers Faust. Chaque côté reçoit ce
  qui lui manque.

**Les limites actuelles, celles qui se travaillent.**

- *Pas de lots ni d'accélérateur.* Une instance traite un signal,
  échantillon par échantillon, sur un cœur ; sur un jeu de données de
  plusieurs heures, on est à des ordres de grandeur d'un framework.
- *Tout est un graphe de signaux.* Une couche de mille poids fait mille
  signaux ; la compilation et la taille du code croissent avec le graphe
  dérivé (quarante tangentes directes à travers une FDN à six lignes :
  15 s). Au-delà de quelques dizaines de milliers de nœuds, la
  différentiation à la compilation ne suit plus.
- *Pas de FFT dans le langage.* La perte spectrale multi-résolution, l'outil
  de base de la DDSP, n'existe pas telle quelle ; les bancs de filtres
  l'approchent.
- *Les bornes de `rad`.* Des bandes proportionnelles au bloc, donc une
  mémoire égale à la longueur du bloc fois les signaux enregistrés ; un
  horizon égal au bloc, adjoint nul à sa fin et rien de transmis d'un bloc au
  suivant, donc exact sur toute une réponse en un `compute` mais tronqué au
  tampon dans un flux, d'où `fad` pour les apprentissages en flux ; pas de
  retard variable (`fad` l'a) ; pas de table en écriture ni de fichier son
  (`rdtable` n'est dérivée que par rapport à son index) ; pas de traversée
  d'une frontière de domaine d'horloge ; la dérivée de la branche active à
  `select2`, `min`, `max`, zéro pour les opérations entières et bit à bit ;
  pas de dérivée seconde (ni `fad` sur `rad` ni `rad` sur `fad`), donc pas de
  produits hessienne-vecteur, même si les colonnes de la jacobienne données
  par `fad` permettent un pas de Gauss-Newton quand les paramètres sont peu
  nombreux.
- *L'outillage.* Pas de graphe d'exécution à inspecter à l'exécution, pas de
  `.grad` sur un nœud ; `faustprobe` rend n'importe quelle voie et le DAG de
  signaux se dumpe, mais trouver l'origine d'un NaN veut dire bissecter la
  source. Aucune planification du taux d'apprentissage n'est fournie (une
  planification est un signal et peut s'écrire), pas de point de reprise
  (l'état d'une instance ne se sauvegarde ni ne se restaure), pas de
  recherche d'hyperparamètres au-delà d'une boucle de l'hôte sur des
  compilations. La double précision reste nécessaire : tangentes et adjoints
  à travers des milliers d'échantillons de récursion perdent vite des
  chiffres en simple précision, alors que les plugins tournent le plus
  souvent en simple.

**Ce que cette approche ne fera pas, par construction.**

- *L'apprentissage profond à grande échelle* : des millions de paramètres,
  des corpus d'heures, les codecs neuronaux, les modèles de diffusion, les
  gros modèles d'ampli à convolutions. La représentation un signal par nœud,
  l'exécution une instance à la fois et l'absence de tenseurs et de GPU les
  excluent ; l'inférence de réseaux moyens en Faust reste possible, pas leur
  entraînement.
- *Les graphes dynamiques* : Faust est un flot de données statique, pas de
  forme dépendant des données, pas de longueur variable autrement que par
  les horloges, pas de récursion sur des structures ; donc pas de
  transformeurs, de recherche en faisceau ni de modèles arborescents.
- *Apprendre des représentations depuis un corpus* : la force de la DDSP
  d'Engel et al. est un encodeur neuronal appris sur des données couplé au
  synthétiseur différentiable ; Faust peut porter la seconde moitié, jamais
  la première.
- *Dériver ce qui n'est pas un signal* : la topologie, le nombre de lignes,
  une longueur de retard entière, un choix discret. Comme dans tout
  framework, cela demande des relaxations, et elles seraient à écrire en
  Faust.

## 8. Références

- J. Engel, L. Hantrakul, C. Gu, A. Roberts, « DDSP: Differentiable Digital
  Signal Processing », ICLR 2020. <https://arxiv.org/abs/2001.04643>
- B. Hayes et al., « A Review of Differentiable Digital Signal Processing for
  Music and Speech Synthesis », Frontiers in Signal Processing, 2024.
  <https://arxiv.org/abs/2308.15422>
- A. G. Baydin, B. Pearlmutter, A. Radul, J. Siskind, « Automatic
  Differentiation in Machine Learning: a Survey », JMLR 2018.
  <https://arxiv.org/abs/1502.05767>
- R. J. Williams, D. Zipser, « A Learning Algorithm for Continually Running
  Fully Recurrent Neural Networks », Neural Computation, 1989.
- S. Haykin, *Adaptive Filter Theory*, Prentice Hall — LMS, NLMS, RLS.
- L. Ljung, T. Söderström, *Theory and Practice of Recursive
  Identification*, MIT Press, 1983 — méthodes récursives d'erreur de
  prédiction.
- J. J. Shynk, « Adaptive IIR Filtering », IEEE ASSP Magazine, 1989 —
  régression pseudo-linéaire contre erreur de prédiction récursive.
- P. L. Feintuch, « An Adaptive Recursive LMS Filter », Proc. IEEE, 1976.
- D. Marquardt, « An Algorithm for Least-Squares Estimation of Nonlinear
  Parameters », SIAM J. Appl. Math., 1963. <https://doi.org/10.1137/0111030>
- D. P. Kingma, J. Ba, « Adam: A Method for Stochastic Optimization », ICLR
  2015. <https://arxiv.org/abs/1412.6980>
- X. Chen et al., « Symbolic Discovery of Optimization Algorithms » (Lion),
  2023. <https://arxiv.org/abs/2302.06675>
- J. Zhuang et al., « AdaBelief Optimizer », NeurIPS 2020.
  <https://arxiv.org/abs/2010.07468>
- P. J. Huber, « Robust Estimation of a Location Parameter », Ann. Math.
  Statist., 1964. <https://doi.org/10.1214/aoms/1177703732>
- J. D. Markel, A. H. Gray, *Linear Prediction of Speech*, Springer, 1976 —
  coefficients de réflexion et forme en treillis.
  <https://ccrma.stanford.edu/~jos/filters/Lattice_Ladder_Filters.html>
- V. Zavalishin, *The Art of VA Filter Design* — rétroaction sans délai et
  solveurs implicites.
- I. Loshchilov, F. Hutter, « SGDR: Stochastic Gradient Descent with Warm
  Restarts », ICLR 2017. <https://arxiv.org/abs/1608.03983>
- Notes côté Faust : [docs/fad-note-en.md](../docs/fad-note-en.md),
  [docs/ondemand-note-fr.md](../docs/ondemand-note-fr.md),
  [docs/rad-note-en.md](../docs/rad-note-en.md),
  [docs/rad-usage-en.md](../docs/rad-usage-en.md),
  [docs/fad-rad-synthesis-fr.md](../docs/fad-rad-synthesis-fr.md),
  [docs/fad-debruijn-recursion-en.md](../docs/fad-debruijn-recursion-en.md).
