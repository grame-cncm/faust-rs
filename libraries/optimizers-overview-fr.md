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

### 2.5 Plus vite que l'audio : horloges entières et `upsampling`

Toutes les horloges de cette bibliothèque sont booléennes : un bloc tourne
zéro ou une fois par échantillon. Les primitives permettent davantage. Une
horloge *entière* exécute le corps `H` fois par échantillon extérieur, `H`
étant un signal, donc variable d'un échantillon à l'autre, et `H = 0`
maintient. À l'intérieur, le temps compte les itérations : une récursion `~`
dans le corps est une **boucle avec état, à l'exécution**, ce que Faust ne
sait pas écrire autrement (`par` et `seq` déroulent à la compilation). Les
entrées sont figées au départ du bloc, les sorties sont celles de la
dernière itération, et l'état survit d'un échantillon au suivant.
`upsampling(C)` ajoute deux choses : ses entrées sont bourrées de zéros
(l'échantillon arrive à la dernière itération, des zéros avant, le `↑₀` de
`interleave.lib`), et `ma.SR` vaut `SR * H` à l'intérieur. `fad` traverse
les deux, état compris, par l'augmentation de bloc de la section 2.4. Quatre
usages en découlent, chacun essayé avec `faustprobe` le 18 septembre 2026 ;
les programmes sont les lignes citées, en double précision à 48 kHz.

**Les solveurs implicites comme boucle.** `newton(N, F, y0)` déroule `N`
pas dans le graphe et repart de `y0` à chaque échantillon. Dans un bloc à
horloge entière, l'itération est une récursion et l'état du bloc *est*
l'itéré : l'échantillon suivant repart de la solution précédente (un
démarrage à chaud gratuit), le nombre de pas est un signal, et le code a la
taille d'un pas :

```faust
F(x, y) = y - ma.tanh(x - fb * y);                    // y = tanh(x - fb y), une saturation à rétroaction
step(x, y) = y - (fad(F(x, y), y) : /);
loop(x) = (K, x) : ondemand(\(xi).(step(xi) ~ _));    // K pas par échantillon, à chaud
```

Contre `newton(8, ...)` depuis 0, qui coûte 10,2 ms par seconde d'audio
(`fb` 0,8, un sinus d'amplitude 1) :

| pas par échantillon | écart, 100 Hz | écart, 2 kHz | coût par seconde d'audio |
|---|---|---|---|
| 1 | 3,4e-6 | 1,9e-3 | 1,6 ms |
| 2 | 1,2e-12 | 3,4e-7 | 3,6 ms |
| 3 | 3e-16 | 1,2e-14 | 5,0 ms |

**Le nombre de pas décidé par la convergence.** L'horloge est calculée hors
du bloc, avant qu'il ne tourne, donc elle ne peut pas être la convergence
de l'itération qu'elle lance ; ce que le compilateur émet est une boucle
`for` comptée, jamais un `while`. Mais un compte peut être décidé à partir
de ce qui est connu à ce moment-là, et un bloc booléen imbriqué peut
arrêter l'itération de l'intérieur. Deux couches. Dehors, le résidu du
démarrage à chaud, `F(x, y_prev)` avec la nouvelle entrée et la solution
maintenue, décide le compte : 0 quand la solution précédente satisfait déjà
la tolérance (le bloc ne tourne pas, la sortie est maintenue, comme avec
`on_change`), un budget `Kmax` sinon. Dedans, un pas par itération tant que
le résidu de l'itéré courant dépasse la tolérance, les itérations non
utilisées du budget ne coûtant que le résidu :

```faust
body(xi) = inner ~ _
with { inner(y) = ((abs(F(xi, y)) > tol), y) : ondemand(\(yp).(step(xi, yp))); };
solve(x) = sel ~ _
with { sel(yp) = ((abs(F(x, yp)) > tol) * Kmax, x) : ondemand(body); };
```

Mesuré avec `tol` 1e-12 et un budget de 8, les pas étant les tirs du bloc
intérieur, égal au solveur déroulé à 1,1e-12 :

| entrée | pas par échantillon, moyenne et maximum | coût par seconde d'audio |
|---|---|---|
| constante | 0 après le premier échantillon | 0,32 ms |
| sinus, 100 Hz | 2,4, 3 | 5,9 ms |
| sinus, 2 kHz | 3,0, 3 | 6,5 ms |
| bruit blanc | 3,5, 5 | 7,4 ms |

Un solveur qui ne coûte rien tant que son entrée ne bouge pas, et les pas
qu'il lui faut sinon. Une estimation du compte à partir de la convergence
quadratique, depuis le résidu du démarrage à chaud, éviterait les tests du
budget inutilisé ; pas essayé. Le gradient d'un paramètre appris traverse le solveur : la
tangente de la solution par rapport à `fb` sur une entrée constante vaut
−0,240418, la différence centrale −0,24042. Deux choses à savoir. La
tangente d'une itération converge vers la dérivée de son point fixe quand
l'itération contracte (Christianson, 1994), donc les lanes de tangente
donnent le bon gradient dès que la primale a convergé ; et avec beaucoup de
paramètres, un seul `fad` du résidu à la solution, la dérivée implicite
−F_p / F_y, coûte moins qu'une lane de tangente par itération. C'est le
modèle analogique appris sur un enregistrement : un écrêteur à diodes, un
ladder à rétroaction sans délai, une saturation rebouclée, dont les
paramètres se dérivent à travers le solveur.

**Les non-linéarités suréchantillonnées.** `upsampling(C)` est à lui seul
l'étage suréchantillonné : le bourrage de zéros par contrat, un filtre
d'interpolation au rythme interne, la non-linéarité, un filtre de
décimation, et la dernière itération comme décimation. Un waveshaper appris
sous une perte spectrale en a besoin : le repliement est une erreur de
spectre, et le gradient apprendrait à l'annuler plutôt que la forme.

```faust
SRraw = fconstant(int fSamplingFreq, <math.h>);
lp = fi.lowpass(6, 20000 * ma.SR / SRraw);            // voir le piège plus bas
stage(x) = (H, x) : upsampling(\(xi).(xi * H : lp : ma.tanh : lp));
```

Mesuré sur un sinus à 5250 Hz (choisi pour que les harmoniques repliées
tombent sur des bins qui ne sont pas des harmoniques), comme rapport de
l'énergie harmonique au reste du spectre sur une fenêtre de 4096
échantillons, à un drive de 8 et, pour la chaîne seule, à un drive de 0,5 :

| facteur | tanh, drive 8 | drive 0,5 | coût par seconde d'audio |
|---|---|---|---|
| `ma.tanh` nu | 13,5 dB | 66,6 dB | |
| 2 | 30,7 dB | 84,7 dB | 2,0 ms |
| 4 | 31,4 dB | 81,1 dB | 3,8 ms |
| 8 | 30,1 dB | 80,0 dB | 7,3 ms |
| 16 | 29,7 dB | 79,7 dB | |

Le plateau à 30 dB est la bande de transition du Butterworth d'ordre 6 à
20 kHz, pas le mécanisme. Le piège : `maths.lib` borne `ma.SR` à 192 kHz,
donc au-delà de ×4 à 48 kHz les filtres de `filters.lib` sont conçus pour
un rythme plus bas que celui du bloc (×8 mesurait 15,6 dB et 58,1 dB avant
la correction). Leur conception ne dépend que de `fc / SR`, donc demander
`fc * ma.SR / SRraw` conçoit le bon filtre. Le facteur peut être un signal,
×4 tant qu'un suiveur d'enveloppe dépasse un seuil et ×1 en dessous : le
programme tourne et reste fini ; le filtre intérieur devient variant dans
le temps à la commutation, ce qui n'a pas été qualifié plus loin. Le
gradient traverse le bloc : la dérivée de la moyenne du carré de la sortie
par rapport au drive rejoint la différence centrale à 2,9e-6 en ×4.

**Plusieurs pas d'optimiseur par tick.** Avec `H = K * frame_clock(N)`,
toute une boucle `descend_*` tourne `K` fois au tick de trame et jamais
entre deux : `K` pas sur l'instantané que le tick livre, à chaud depuis la
trame précédente, ce qui est l'analyse par synthèse par trame du DDSP.

```faust
learn(x) = (K * il.frame_clock(64), x)
         : ondemand(\(xi).(op.descend_1D(\(p).(op.mse(p * xi, 0.7 * xi)), op.sgd_g(0.1), -4, 4, 0, 0)));
```

Mesuré sur un gain depuis 0 avec SGD à 0,1 : avec `K = 1` le paramètre est à
0,692 après 20 trames ; avec `K = 4` après 5 trames, et à 0,7 à 1e-8 près
après 20 ; avec `K = 16` à 0,7 à 1e-8 près après 5. Le prix est l'endroit où
le travail tombe : sur un seul échantillon (la ligne « worst block » de
`faustprobe --time`), là où la forme booléenne l'étale sur la trame au prix
d'une trame de latence.

**Une recherche comme boucle.** Une grille de `M` candidats essayés un par
itération, le candidat fonction du temps local du bloc, l'état gardant le
meilleur : un code de la taille d'une évaluation, là où `multistart_1D` et
`grid_then_descend_1D` copient le graphe `M` fois.

```faust
body(xi) = best ~ (_, _)
with {
    i = ((+(1)) ~ _) - 1 : %(M);                  // le temps local : l'indice d'itération
    cand = -1.0 + 2.0 * i / (M - 1);
    best(pb, lb) = select2(better, pb, cand), select2(better, lb, l)
    with { l = loss(cand, xi); better = (l < lb) | (i == 0); };
};
grid = (M * clock, _) : ondemand(body);
```

Seize candidats sur [−1, 1] pour une cible à 0,37 élisent 0,333 en un tick.
La même boucle est une recherche linéaire à rebours (diviser le pas jusqu'à
ce que la perte baisse, avec arrêt à l'exécution), les deux évaluations
d'un pas SPSA dans un échantillon au lieu de deux trames, ou un
redémarrage.

**Limites.** Les sorties sont celles de la dernière itération, jamais une
par itération. L'horloge est calculée dehors, donc un critère d'arrêt venu
de l'intérieur passe par un bloc booléen imbriqué, comme ci-dessus.
L'horloge est opaque à la dérivée, ce qui est juste (un nombre d'itérations
est discret), et `rad` ne traverse pas la frontière : l'apprentissage à
travers ces blocs se fait en `fad` seulement. Le mode vectoriel ne couvre
pas les blocs d'horloge. Rien de ceci n'est encore empaqueté :
`newton_loop`, `oversampled`, `descend_*_burst` et `grid_seq` en sont les
candidats.

### 2.6 Ce que ce n'est pas

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

Le fichier [optimizers.lib](optimizers.lib) (préfixe `op`, version 0.9.0) est
documenté fonction par fonction selon la convention des bibliothèques Faust ;
cette section en donne la carte. Il comporte quinze sections, ordonnées des
briques de base aux boucles prêtes à l'emploi.

| Section | Contenu | Raison d'être |
|---|---|---|
| Signal helpers and parameter state | `clip`, `sgn`, `ema`, `ema_bc`, `pstate`, `polyak`, `init_latch`, `init_reset`, `stalled`, `no_progress` | les quelques primitives avec lesquelles tout moteur et toute boucle sont écrits, par-dessus `si`, `ba`, `ro`, `ma` ; démarrer une boucle sur une estimation extérieure ; détecter un plateau plat (`stalled`) ou en pente (`no_progress`) |
| Losses and regularizers | `mse`, `pseudo_huber`, `logcosh`, `energy_loss`, `log_energy_loss`, `corr_loss`, `bank_log_energy_loss`, `frame_spectral_loss`, `l2`, `l1s` | une perte est une fonction Faust ordinaire `loss(y, t)` ; celles-ci sont lisses ; les trois dernières comparent des sons plutôt que des formes d'onde et élargissent un bassin |
| Reparameterizations | `poles_from_reflection`, `reflection_from_poles`, `sigmoid_map` | apprendre dans un domaine où toute valeur est admissible (stable, positive, bornée) plutôt que borner |
| Gradient conditioning and schedules | `clip_g`, `softclip_g`, `gate_g`, `ramp_lin`, `ramp_exp`, `lr_exp`, `lr_cos`, `warmup` | ce qui arrive à un gradient avant le moteur, et comment une vitesse d'apprentissage, ou un paramètre du modèle, évolue |
| Least-squares engines | `lms`, `nlms`, `gn1`, `sgd`, `adam`, `rmsprop`, `nadam`, `sign_sgd` | moteurs qui voient séparément le résidu `r` et la sensibilité `j` |
| Gradient engines | `sgd_g`, `momentum_g`, `nesterov_g`, `adam_g`, `nadam_g`, `amsgrad_g`, `adabelief_g`, `rmsprop_g`, `adagrad_g`, `lion_g`, `sign_g`, `langevin_g` | moteurs qui voient un seul nombre, le gradient de la perte `g` ; `langevin_g` y ajoute un bruit recuit pour quitter un puits peu profond |
| Least-squares loops | `lsq_1D` … `lsq_5D`, `optimize_1D` … `optimize_5D`, `lsq_1D_restart` | le modèle est différencié, la perte est implicitement l'erreur quadratique ; `lsq_1D_restart` change de départ quand le résidu ne progresse plus |
| Loss-first loops | `descend_1D` … `descend_5D`, `descend_1D_restart` | la perte est différenciée, quelle qu'elle soit ; `descend_1D_restart` prend le départ suivant quand la perte ne progresse plus |
| Gauss-Newton loops | `lm_2D`, `lm_3D` | pas de second ordre pour deux ou trois paramètres corrélés |
| Bus loops | `lsq_N`, `descend_N`, `descend_N_clocked` et `lsq_N_rad`, `descend_N_rad`, `descend_N_rad_clocked` | `N` paramètres en bus avec un moteur et une paire de bornes, en mode direct ou inverse |
| Clocked loops | `frame_sum`, `frame_count`, `frame_mean`, `descend_1D_clocked` … `descend_5D_clocked` | le gradient à cadence audio, moyenné sur la trame, le pas une fois par tir d'une horloge `ondemand` |
| Gradient-free loops | `spsa_1D_clocked`, `spsa_N_clocked`, `search_1D_clocked` | apprendre sans aucune tangente, par deux évaluations de la perte par trame : un retard entier, un `select2`, tout ce que `fad` dérive à zéro |
| Multi-start loops | `grid_init`, `multistart_1D`, `multistart_lsq_1D`, `grid_then_descend_1D` | plusieurs départs à la fois : `K` descentes en parallèle dont on suit la meilleure, ou `K` candidats notés sans tangente puis une descente depuis le meilleur |
| Porte et arrêt | `gated`, `gated_when`, `stop_after`, `stop_below`, `stop_relative`, `on_change` | couper l'apprentissage une fois qu'il a convergé, pour qu'il ne coûte plus rien ensuite ; ne calculer des coefficients que lorsqu'un paramètre change |
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
| `langevin_g` | Welling & Teh, 2011 — dynamique de Langevin à gradient stochastique | le pas SGD plus un bruit d'écart-type `sqrt(2 lr temp)` : à température fixe le paramètre échantillonne `exp(-perte / temp)`, recuite vers zéro il explore puis descend. Mesuré sur une perte à deux puits, `(p² - 1)² + 0,3 p` depuis le puits peu profond : SGD y reste (0,960), Langevin passe la barrière et refroidit dans le puits profond (-1,036) ; à température nulle, identique à SGD bit pour bit |

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
audible sur un paramètre. Un schedule est un signal : `ramp_lin` et
`ramp_exp` sont les mêmes rampes sous un nom neutre (`lr_exp` est
`ramp_exp`, bit pour bit), pour recuire un paramètre du *modèle* — un
amortissement, le lissage d'une perte — et non seulement une vitesse ; c'est
la continuation de la section 9, écrite comme un signal. `init_latch` et
`init_reset` font d'une estimation extérieure l'`init` d'une boucle : la
boucle est tenue à `init` pendant que l'estimation s'observe, puis relâchée
la valeur figée ; `stalled` lit un plateau plat, gradient petit sous une
perte haute, et `no_progress` un plateau en pente, une perte qui ne baisse
plus sur une fenêtre de patience : c'est sur ce dernier que
`descend_1D_restart` passe au départ suivant, car sur la corde le gradient
est plus grand sur le plateau que dans le puits (section 5). `gate_g` n'apprend que lorsqu'une condition est
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
- **Pertes spectrales multi-échelles** : la bibliothèque en a deux formes
  depuis le 16 septembre 2026, `bank_log_energy_loss` par banc de filtres à
  cadence audio et `frame_spectral_loss` par trame pour un corps `ondemand` ;
  la version à plusieurs tailles de trame se compose de deux blocs et n'est
  pas emballée, l'arité d'un bloc étant sa taille de trame.

### 4.9 Porte et arrêt

Un bloc qui a convergé continue de coûter ce qu'il coûtait en apprenant :
le modèle qui porte les tangentes, la perte et le moteur tournent à chaque
échantillon. La seule façon de ne pas calculer quelque
chose en Faust est un domaine d'horloge, puisque `select2` évalue ses deux
branches et que `gate_g` met à zéro un gradient déjà calculé ;
`ondemand(C)` ne calcule rien tant que son horloge se tait et tient ses
sorties. `gated(C)` est cela, appliqué à un bloc dont la dernière sortie
est un drapeau : l'horloge vaut `1 - flag'`, le bloc tourne à chaque
échantillon jusqu'à ce que le drapeau se lève, puis plus jamais. Le retard
d'un échantillon de la récursion est ce qui rend la construction légale
(une horloge ne peut pas dépendre de la sortie du bloc au même échantillon)
et ce qui fait qu'un drapeau levé au dernier échantillon d'une période
arrête le bloc à la frontière de période. `gated_when` ajoute un signal
d'activation ; les deux ont besoin d'`outputs(C)`, d'où leur écriture avec
`route` plutôt qu'à arité fixe.

Le drapeau est l'affaire du critère, pas de la porte, d'où les fonctions
`stop_*` séparées, toutes bâties sur `frame_sum` cadencé par la période et
sur `ba.peakhold(1)`, le maximum courant de la bibliothèque standard, qui
tient un drapeau levé : un budget de périodes (`stop_after`), un seuil de perte
(`stop_below`), et `stop_relative`, qui compare la perte d'une période à la
perte du point de contrôle précédent, `window` périodes plus tôt, et
s'arrête quand leur changement relatif est sous `tol`, après `min_periods`,
ou à `max_periods`. Les périodes consécutives ne sont pas comparées : sur
une perte qui dépasse, son plateau ressemble à une convergence pendant
quelques périodes et un test à une période s'y déclenche ; les points de
contrôle sont ce qui rend le test robuste.

`on_change(C)` est l'autre moitié de l'économie. Une fois arrêté, les
paramètres appris restent des signaux, et un filtre qui en calcule ses
coefficients recalcule `exp`, `tan` ou `cos` à chaque échantillon, là où le
compilateur sort les mêmes expressions de la boucle d'échantillons quand
elles dépendent de sliders. `on_change` fait tourner `C` dans un `ondemand`
dont l'horloge est la comparaison de chaque entrée avec sa valeur
précédente, plus le premier échantillon : une fois par pas d'optimiseur
pendant l'apprentissage, plus jamais ensuite. Il faut des filtres qui
prennent des coefficients plutôt que des paramètres ;
`fi.filterbank(1, (fx)) : *(g), _ :> _` est le shelf de
`fi.highshelf(1, L, fx)` avec un gain linéaire. La section 7 donne les
mesures : la porte divise par environ neuf le coût d'une réverbération
auto-calibrante, `on_change` le ramène à celui de la réverbération seule.

### 4.10 Sans gradient : `spsa_1D_clocked`, `spsa_N_clocked`, `search_1D_clocked`

Tout ce qui précède dérive. Ces trois boucles ne dérivent jamais : la perte
est évaluée à deux valeurs du paramètre sur chaque trame, sur la même
excitation, et la mise à jour prend la différence. La perturbation simultanée
de Spall (1992) tient le signe `delta` sur la trame, évalue `L(p + c delta)`
et `L(p - c delta)`, et donne `(L+ - L-) / (2 c delta)` au moteur, le contrat
ordinaire ; sur une perte quadratique l'estimation est la dérivée exacte de la
trame, et la boucle suit `descend_1D_clocked` à l'arrondi près (section 5).
Pour `N` paramètres, un vecteur de `N` signes et toujours deux évaluations,
là où des différences finies par coordonnée en demanderaient `2N`. La
stratégie d'évolution (1+1) de Rechenberg (1973) tient un candidat
`p + sigma u` à côté du titulaire et le garde quand sa perte de trame est
strictement plus basse ; pas de moteur, le pas est l'acceptation. Ce qu'elles
atteignent, c'est ce que `fad` ne voit pas : une longueur de retard entière
(mesuré : un peigne dont le retard entier va de 160 à 200 échantillons), un
`select2` sur le paramètre (mesuré : la branche trouvée en quelques trames
quand `descend_1D` ne bouge pas), une table écrite. Le prix : deux copies du
modèle au lieu d'une copie et sa tangente, un pas par trame, et une
estimation bruitée qui demande un `c` ou un `sigma` à l'échelle du paramètre.

### 4.11 Plusieurs départs : `multistart_1D`, `multistart_lsq_1D`, `grid_then_descend_1D`

Quand aucune estimation ne dit dans quel bassin est la réponse, on part de
plusieurs endroits. `multistart_1D` et sa jumelle moindres carrés font
tourner `K` descentes en parallèle, chacune avec son moteur, et suivent celle
dont la perte lissée est la plus basse, la première en cas d'égalité ; le prix
est `K` modèles et leurs tangentes. Mesuré sur la corde : quatre boucles NLMS
depuis 176, 200, 228 et 264 Hz, seule celle de 228 Hz verrouille, et la
boucle la suit dès 16 000 échantillons. `grid_then_descend_1D` note `K`
candidats fixes pendant `T` échantillons sans aucune tangente, puis lance une
seule descente depuis le meilleur, figé par `init_latch` : le schéma
détecteur puis gradient de la DDSP, écrit à la main, pour `K` modèles
pendant la fenêtre puis un modèle et sa tangente (les candidats continuent
de tourner : une branche figée n'est pas élaguée). La grille ne voit un bassin
que si son pas est plus fin que lui : sur la corde, dont le puits fait ±1 Hz,
une grille sur toute la plage demanderait des centaines de cellules, là où
quatre départs répartis en comprennent un dans la zone de capture ; elle est
mesurée sur la perte à deux puits, où huit cellules suffisent.

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
| `init_latch` + `init_reset` sur la corde, estimation par autocorrélation observée 8 192 échantillons | init figé à 222,77 Hz (+1,3 %), hauteur 219,998 à 24 000, `220,000000` dès 48 000, résidu sous 1e-6 |
| `stalled(0,999, 0,01, 0,1)` sur (gradient, perte) = (0,5, 1), (0, 1), (0, 0,001) | 0, 1, 0 par segment |
| `lr_exp` contre `ramp_exp` | identiques bit pour bit |
| `langevin_g`, température recuite 0,5 → 0, contre `sgd_g`, perte à deux puits depuis le puits peu profond | SGD 0,960 (puits peu profond), Langevin -1,036 (puits profond) à 200 000 échantillons ; à température 0, identique à SGD |
| `spsa_1D_clocked` contre `descend_1D_clocked`, gain, SGD 0,5 par trame de 64 | mêmes trajectoires, différence `0` à chaque échantillon, 0,700000 à 4 000 |
| `spsa_1D_clocked`, retard entier d'un peigne, `c = 2`, Adam 0,5 par trame de 256, depuis 160 | `int(d) = 200` dès 25 000 échantillons, tenu à partir de 40 000, résidu 0 ; la tangente `fad` est identiquement nulle |
| `search_1D_clocked` contre `descend_1D`, `select2(p > 0,5, …)` depuis 0 | la recherche à 0,83 en quelques trames, perte 0 ; `descend_1D` ne bouge pas |
| `descend_1D` + Adam 0,02 sur la corde, gradient et perte lissés | depuis 228 Hz : verrouillé à 220 dès 18 000 échantillons, gradient 0,003, perte 1e-4 ; depuis 200 Hz : marche entre 196 et 207 Hz, gradient 0,025, perte 0,011, plus grand sur le plateau que dans le puits |
| `multistart_lsq_1D(4, (176, 200, 228, 264 Hz))` + NLMS sur la corde | l'index 2 (228 Hz) dès 16 000 échantillons, `220,000000` Hz ; quatre cordes et leurs tangentes compilent en 58 ms |
| `multistart_1D(4, grid_init(4, -3, 3))` + SGD sur la perte à deux puits | les deux départs de gauche finissent dans le puits profond, la boucle suit l'un d'eux, -1,036 |
| `grid_then_descend_1D(8, 2 000, grid_init(8, -3, 3))` sur la perte à deux puits | la cellule -1,125 (index 2) choisie à 2 000 échantillons, descente à -1,036 |
| `descend_1D_restart(2, (1, -1))` + SGD sur la perte à deux puits, patience 4 000 | 0,960 (puits peu profond, perte 0,29 > `eps_l`) puis redémarrage à 8 000 et -1,036 (puits profond) pour de bon |
| `lsq_1D_restart(2, (1, -1))` + NLMS sur `x · L(p)` contre `-0,2 x`, `L` le polynôme à deux puits | 0,960 (résidu 0,24 E[x²], jamais nul dans le puits peu profond) puis redémarrage à 8 000 et une racine de `L = -0,2` dans le puits profond, résidu nul |
| redémarrage sur la corde depuis 228 Hz après une dérive, NLMS ou Adam, simple ou double précision | ne verrouille pas comme une boucle neuve partie de 228 : le modèle, sa tangente et le moteur gardent l'état de la dérive ; non retenu comme fixture |
| `no_progress(2 000, 0,05, 0,1)` sur une perte décroissante, constante haute, constante basse | 0, 1, 0 par segment |
| paysage de la corde balayé de 150 à 300 Hz (`opt_landscape_string.dsp`) | `mse` et `corr_loss` : un puits de ±1 Hz sur un plateau ; `bank_log_energy_loss` (8 bandes, 150–4 800 Hz) : pente monotone vers 220 Hz d'environ 218 à 226 Hz, des extrema locaux à 216 et 232 Hz où les harmoniques s'alignent ; 16 ou 32 bandes ne l'élargissent pas |
| la corde depuis 224 Hz, `bank_log_energy_loss` + SGD 1e-4 contre `mse` + NLMS | `220,000` Hz par le banc à 300 000 échantillons, `220,000` par la forme d'onde aussi, portée par la pente de son plateau (elle capture par le haut jusqu'à 228 Hz, dérive par le bas) ; depuis 200 ou 214 Hz les deux échouent, les alignements d'harmoniques à 214 et 216 Hz bloquant le banc ; SGD 5e-4 sur le banc oscille (la vitesse doit rester sous `1 - a`) |
| `fad` de `bank_log_energy_loss` par rapport à un gain contre une différence finie | -43,06843 contre -43,06874, écart relatif 7e-6 |

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
- **Un `init` calculé dans le graphe a un jour alourdi la compilation.** Un
  `init` de trente voies d'autocorrélation multipliait par cent le temps de
  compilation d'une boucle qui se fermait dessus ; c'était trois parcours non
  mémoïsés du compilateur, corrigés le 16 septembre 2026, et toute boucle
  accepte désormais un tel `init`. Les boucles à un paramètre (`lsq_1D`,
  `descend_1D`, `descend_1D_clocked`) le prennent par un fil d'entrée depuis
  0.9.0, la forme la plus propre.
- **`rad` dans une boucle ne voit qu'un échantillon.** À travers une récursion
  il renvoie le terme direct, pas la dérivée à travers la récursion (section
  4.7) ; apprendre les modèles récursifs avec les boucles `fad`, les modèles
  sans récursion avec les unes ou les autres.

## 7. Deux phases : apprendre, puis servir

Un programme qui apprend dans le processus audio continue de payer son
apprentissage une fois les paramètres stabilisés : le réseau qui porte les
tangentes, la perte et l'optimiseur tournent à chaque échantillon, utiles
ou non, et avec une voie augmentée par paramètre ils font l'essentiel du
coût : pour une douzaine de paramètres, l'effet lui-même n'en est que
quelques pour cent. Couper le gradient avec `gate_g` n'y change rien : il est
calculé, puis mis à zéro. Un `select2` non plus, Faust évalue ses deux
branches. Ce qui le permet, c'est `ondemand` : un bloc dont l'horloge ne
tire pas ne calcule rien et tient ses sorties.

**La bascule.** `gated(C)` (section 4.9) fait tourner un bloc arbitraire
`C`, dont la dernière sortie est un drapeau, dans un `ondemand` externe dont
l'horloge est `1 - flag'` ; tant que le drapeau vaut 0 le bloc tourne à
chaque échantillon, dès qu'il vaut 1 plus rien n'en est calculé et ses
sorties tiennent. Mettre tout l'apprentissage dans `C`, boucle comprise, et
laisser un critère `stop_*` lever le drapeau ; l'effet qui traite l'audio
lit les paramètres tenus et rien d'autre ne change :

```faust
learn(t) = ps <: (si.bus(P), (loss(t) : op.stop_relative(clock, 20, 40, 300, 0.02)))
with { ps = op.descend_N_clocked(P, clock, loss(t), op.adam_g(0.03, 0.9, 0.999, 1e-8), lo, hi, 0.0, 0.0); };
params = t : op.gated(learn);      // P paramètres tenus, puis le drapeau
```

`stop_relative` compare la perte d'une période à celle du point de contrôle
précédent, `window` périodes plus tôt, et s'arrête en fin de période quand
le changement relatif est sous `tol`, après `min_periods`, ou à
`max_periods` ; comparer des périodes consécutives se fait piéger par les
plateaux d'un dépassement. `stop_after` et `stop_below` sont les budgets
plus simples, un nombre de périodes ou un seuil de perte, et `gated_when`
ajoute un signal d'activation. Le retard d'un échantillon de la récursion
fait retomber l'horloge à l'échantillon qui suit le dernier de la période,
si bien que le temps propre du bloc, qui compte ses tirs, reste aligné sur
la période si l'apprentissage reprend. Les pertes par période du programme
cadencé sont bit-identiques à celles du programme non cadencé : un `fad` et
une récursion dans un `ondemand` imbriqué dans un autre `ondemand` sont
compilés exactement.

**Les coefficients.** Une fois arrêté, les paramètres tenus restent des
signaux, donc un filtre qui en calcule ses coefficients recalcule `exp`,
`tan` et `cos` à chaque échantillon, là où le compilateur sort les mêmes
expressions de la boucle quand elles dépendent de sliders. `on_change(C)` comble
l'écart : il fait tourner `C`, le calcul des coefficients (gains des
shelves et de l'égaliseur, cosinus et sinus des angles appris), dans un
`ondemand` dont l'horloge est la comparaison de chaque entrée avec sa valeur
précédente, il tire donc une fois par pas d'optimiseur et plus jamais une
fois l'apprentissage arrêté ; les filtres reçoivent les gains linéaires
tenus (`fi.filterbank(1, (fx)) : *(g), _ :> _` est ce sur quoi
`fi.highshelf` est construit). La réponse est bit-identique à la version
par échantillon.

**Mesuré** sur un cœur, blocs de 320 échantillons, une réverbération à une
douzaine de paramètres appris (l'exemple de calibration du document des
exemples DDSP, poussé plus loin) :

| phase | × temps réel |
|---|---|
| apprentissage, programme non cadencé | 28 |
| apprentissage, programme cadencé | 23 |
| après la bascule | 196 |
| après la bascule, coefficients hissés | 572 |
| le même réseau avec des sliders, sans apprentissage | 625 |
| après une recompilation avec les valeurs apprises en constantes | 631 |

La bascule coûte environ 18 % pendant l'apprentissage, l'imbrication des
domaines ; une fois arrêté, l'effet ne coûte pas plus que le même réseau
seul. La dernière ligne est l'autre voie, celle qu'un framework à tenseurs
est obligé de prendre : l'hôte remplace les sliders par les valeurs
apprises, recompile (0,07 s avec le JIT Cranelift) et remplace l'instance,
qui part alors du silence et doit être fondue ; la propagation de
constantes ne gagne rien sur les sliders, les expressions qui en dépendent
sortant déjà de la boucle d'échantillons. La bascule n'a besoin d'aucun
hôte et garde l'état ; c'est `ondemand` appliqué à la dérivée, possible
parce que la dérivée est un signal du même programme, et des horloges
décident de ce qui en est calculé : l'apprentissage tant qu'il sert, les
coefficients quand un paramètre change, l'effet toujours.

## 8. Portée : ce que cela atteint, et ce que cela n'atteint pas

Face aux frameworks à tenseurs de la DDSP (PyTorch ou JAX avec les
bibliothèques DDSP, torchaudio, FLAMO, dasp-pytorch), la différentiation au
niveau du compilateur est à la DDSP ce que le filtrage adaptatif est à
l'apprentissage automatique : exacte, bon marché, temps réel, interprétable,
petite. Son domaine, ce sont les modèles paramétriques dont un ingénieur du
son sait lire les paramètres. La calibration d'une réverbération sur des
salles mesurées, hors ligne et dans le processus audio, est l'exemple
travaillé derrière cette section, qui est ce qu'il a appris sur la portée de
l'approche.

**Ce qu'on peut raisonnablement atteindre.**

- *Calibration et identification de systèmes* : réverbérations, filtres,
  égaliseurs, modèles physiques, circuits à boîte grise, avec des dizaines à
  quelques centaines de paramètres. `rad` coûte environ trois passes avant
  quel que soit leur nombre, donc quelques centaines restent abordables hors
  ligne.
- *L'apprentissage dans le processus audio*, ce qu'aucun framework ne fait :
  effets qui se calibrent, suivi d'une cible qui dérive, annulation d'écho,
  filtres adaptatifs, patches qui s'accordent, et l'apprentissage coupé par
  une horloge `ondemand` une fois fini, sans coût ensuite (section 7). Réaliste jusqu'à
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
  dérivé (quarante tangentes directes à travers une petite réverbération :
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

## 9. Non-convexité : ce que le gradient demande au paysage

La descente de gradient ne garantit le minimum global que pour une perte
*convexe* : un seul bassin, vers lequel tout point de départ descend.
Presque aucune perte de ce document ne l'est, et pourtant presque tous les
programmes convergent. La convexité n'est donc pas ce qui sépare ce qui
apprend de ce qui n'apprend pas. Trois questions le font : le point de
départ est-il dans le bassin du bon minimum ; le gradient y est-il
informatif, ou le paysage est-il plat ; le problème est-il conditionné,
c'est-à-dire les paramètres ont-ils des échelles comparables. Cette section
relit les exemples du dépôt à travers ces trois questions, puis dit ce que
la bibliothèque offre quand le paysage est difficile, et ce qu'elle n'offre
pas.

**Ce qui est convexe.** Un modèle *linéaire en ses paramètres* sous une
erreur quadratique donne une perte quadratique, un bol : un gain, un biais,
les coefficients d'un FIR, les amplitudes d'un banc d'harmoniques. C'est le
domaine du LMS et de ses parents, l'annuleur d'écho à 64 coefficients, les
boucles à bus, le synthétiseur harmonique. Ces programmes convergent depuis
n'importe quel point de départ ; seule la vitesse dépend du conditionnement,
que la normalisation de `nlms` règle. Même là un piège subsiste : une perte
sur des *magnitudes* est aveugle au signe et a deux minima symétriques, `a`
et `−a` ; l'exemple 11 de `ddsp-examples-fr.md` en supprime un avec
`a_h = exp(p_h)`, une reparamétrisation qui ne laisse qu'un minimum.

**Ce qui ne l'est pas.** Dès qu'un paramètre entre dans une récursion ou
dans une fréquence, la perte cesse d'être convexe : la fréquence et le Q
d'un résonateur, les pôles d'un biquad, le `c = cos w` d'un notch, le T60
d'un FDN, la longueur de retard d'une corde, les poids d'un GRU ou d'un MLP.
L'erreur de sortie d'un filtre récursif peut avoir des minima locaux
(Stearns 1981 ; Söderström & Stoica 1982), surtout quand le modèle est
d'ordre insuffisant. Pourtant le notch converge de 1400 Hz à 1000, `resonlp`
de `(1000, 1)` à `(1200, 2)`, le FDN de `(0,3 s, 0)` à `(0,6, 0,3)`, le
biquad de zéro à sa cible. Ce n'est pas la convexité qui les sauve : c'est
un bassin assez large et un point de départ dedans.

**Le contre-exemple mesuré.** La corde à guide d'onde (exemple 10 de
`ddsp-examples-fr.md`) montre le paysage lui-même. L'erreur de forme d'onde
entre deux cordes est un puits de ±1 Hz de large autour de 220 Hz sur un
plateau plat. Depuis 228 Hz la hauteur se cale à 220,000000 ; depuis 200 Hz
elle dérive à 190. Même modèle, même perte, même optimiseur : seul le point
de départ change. C'est la raison pour laquelle le DDSP d'Engel et al. fait
estimer f0 par un détecteur et laisse le gradient affiner ; le DDSP n'est
pas une méthode pour problèmes convexes, c'est un ensemble de techniques
pour rendre un paysage non convexe praticable par le gradient.

**Ce que la bibliothèque offre pour un paysage difficile.** Chaque outil
agit sur l'une des trois questions.

- *Le point de départ.* L'`init` de chaque boucle est le premier outil, et
  le plus fort : une estimation extérieure, un détecteur de hauteur, la
  valeur d'une session précédente. `init_latch(T, e)` et `init_reset(T)`
  font d'une estimation observée `T` échantillons cet `init` : sur la corde,
  le pic d'autocorrélation de la cible, figé 2 % au-dessus, amène la hauteur
  à `220,000000` sans qu'aucun départ soit choisi à la main (section 5).
  `on_change` et l'entrée `reset` permettent de repartir quand la cible
  saute.
- *La reparamétrisation.* Elle change la forme du paysage sans déplacer son
  minimum. La fréquence en log rend les pas relatifs ; les coefficients de
  réflexion transforment le triangle de stabilité en boîte, donc tout point
  atteint est un filtre valide ; `exp` sur une amplitude supprime le minimum
  miroir ; `sigmoid_map` remplace une borne dure, où le gradient se perd,
  par une pente.
- *La perte.* Une erreur de forme d'onde compare des phases, d'où des puits
  étroits ; `energy_loss` et `log_energy_loss` comparent des puissances
  lissées et élargissent le bassin. La section 7.2 du tutoriel le mesure :
  sur deux excitations indépendantes, `mse` laisse la coupure bloquée sur
  la borne de 20 Hz, `log_energy_loss` la ramène entre 770 et 850 Hz autour
  des 800 Hz de la cible. `bank_log_energy_loss` fait de même par bande sur
  un banc de filtres : sur la corde, le puits de ±1 Hz de la forme d'onde
  devient une pente vers 220 Hz d'environ 218 à 226 Hz, que SGD descend
  depuis 224 Hz ; sur cette corde cela n'achète pourtant pas un départ que la
  forme d'onde ne sait pas traiter, sa pente de plateau capturant par le haut
  et les alignements d'harmoniques bloquant les deux par le bas (section 5) ;
  `corr_loss` retire le biais de
  puissance mais n'élargit pas le puits ; `frame_spectral_loss` est la forme
  par trame pour un corps `ondemand`. Les pertes robustes (`logcosh`,
  `pseudo_huber`) ne changent pas la forme du bassin, elles bornent les
  coups que les aberrants lui portent.
- *La continuation.* Commencer sur un paysage lisse et le durcir en cours
  de route : l'amortissement de la corde recuit de 0,70 à 0,95 (résonances
  larges d'abord) fait converger depuis 264 Hz ce qui ne convergeait que
  depuis 228 ; le rayon `r` du notch fixe de même la largeur du bassin
  (0,9 large, 0,99 étroit). `ramp_lin` et `ramp_exp` écrivent ce recuit
  comme un signal ; un schedule de vitesse, `lr_exp` ou `lr_cos`, en est la
  version la plus simple : explorer vite, puis se poser.
- *Le second ordre.* `lm_2D` et `lm_3D` règlent le conditionnement, pas la
  multimodalité : un pas de Gauss-Newton descend dans le bassin où il se
  trouve, seulement plus vite et sans vitesse par paramètre.
  L'amortissement de Marquardt est ce qui le garde raisonnable là où
  l'approximation quadratique est fausse, loin de la solution.
- *Le terme direct.* Les boucles `_rad` (section 4.7) ne changent pas le
  paysage non plus : leur convergence repose sur une condition de
  positivité, pas sur la forme de la perte.

**Quand le gradient ne suffit plus.** Un paysage à plusieurs bassins sans
bonne initialisation demande une recherche que le gradient ne fait pas, et
qui en général l'encadre plutôt qu'elle ne le remplace :

- *plusieurs départs* : la même descente lancée depuis des points distincts,
  on garde celle dont la perte lissée est la plus basse ;
- *une grille grossière puis le gradient* : balayer le paramètre difficile,
  la hauteur ou une longueur de retard, à pas larges, et affiner par
  descente depuis le meilleur point ; c'est le schéma détecteur puis
  gradient de la DDSP, écrit à la main ;
- *les méthodes sans gradient* : recuit simulé, CMA-ES (Hansen 2016),
  Nelder-Mead, optimisation bayésienne ; elles n'évaluent que la perte et
  conviennent à peu de paramètres, hors ligne, là où le gradient est nul ou
  trompeur ;
- *les paramètres discrets* : une longueur de retard entière, une topologie,
  un choix ; ils n'ont pas de dérivée (section 8), il faut les relaxer ou
  les énumérer.

De tout cela, la bibliothèque a les briques — l'`init` sur estimation, les
rampes, le détecteur de plateau `stalled` (gradient petit sous une perte
haute), `langevin_g`, le pas SGD plus un bruit recuit, qui quitte un puits
peu profond (section 5) mais n'attire pas sur un plateau — et les boucles
sans gradient de la section 4.10, `spsa_1D_clocked`, `spsa_N_clocked` et
`search_1D_clocked`, qui apprennent les paramètres discrets par deux
évaluations de la perte par trame ; et `descend_1D_restart`, le multi-start
séquentiel au prix d'un seul modèle, qui prend le départ suivant quand la
perte ne progresse plus (mesuré sur les paysages à deux puits : le puits peu
profond quitté à 8 000 échantillons pour le profond ; sur la corde, un
départ pris après une dérive ne verrouille pas comme une boucle neuve,
section 5) ; et les départs multiples de la section 4.11,
`multistart_1D`, `multistart_lsq_1D` et `grid_then_descend_1D`, les deux
formes que ce paragraphe annonçait. Dans le graphe, `multistart_1D` est
la première telle quelle, `K` boucles en parallèle, une perte lissée par
`ema_bc` pour chacune et un sélecteur qui suit la meilleure ; éteindre les
perdantes avec `gated` reste une composition à écrire à la main. Côté hôte,
la boucle de [docs/rad-usage-en.md](../docs/rad-usage-en.md) recompile et
écrit les paramètres par `set_real_zone`, et un multi-start ou une recherche
bayésienne sur les inits s'y greffe toujours sans toucher au compilateur.

**Convexe ne veut pas dire facile.** En ligne, un échantillon à la fois, un
bol quadratique se traverse aussi mal qu'un autre paysage si le pas est mal
choisi : la marche aléatoire d'Adam sur une perte bruitée, l'oscillation
d'une perte lissée plus lente que l'optimiseur, un paramètre en hertz et un
autre sans unité sous une seule vitesse. Ces murs sont ceux de la section 6
et de la section 13 du tutoriel, et ils n'ont rien à voir avec la
convexité.

## 10. Références

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
- S. D. Stearns, « Error Surfaces of Recursive Adaptive Filters », IEEE
  Trans. ASSP, 1981 — minima locaux de l'erreur de sortie d'un filtre
  récursif.
- T. Söderström, P. Stoica, « Some Properties of the Output Error Method »,
  Automatica, 1982 — unimodalité et minima locaux de l'erreur de sortie.
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
- B. Christianson, « Reverse accumulation and attractive fixed points »,
  Optimization Methods and Software, 1994 — la dérivée d'une itération
  contractante converge vers la dérivée de son point fixe.
  <https://doi.org/10.1080/10556789408805572>
- I. Loshchilov, F. Hutter, « SGDR: Stochastic Gradient Descent with Warm
  Restarts », ICLR 2017. <https://arxiv.org/abs/1608.03983>
- M. Welling, Y. W. Teh, « Bayesian Learning via Stochastic Gradient Langevin
  Dynamics », ICML 2011 — le moteur `langevin_g`.
- J. C. Spall, « Multivariate Stochastic Approximation Using a Simultaneous
  Perturbation Gradient Approximation », IEEE Trans. Automatic Control, 1992
  — `spsa_1D_clocked`, `spsa_N_clocked`.
- H.-G. Beyer, H.-P. Schwefel, « Evolution Strategies: A Comprehensive
  Introduction », Natural Computing, 2002 — `search_1D_clocked`.
- N. Hansen, « The CMA Evolution Strategy: A Tutorial », 2016 — recherche
  sans gradient. <https://arxiv.org/abs/1604.00772>
- Notes côté Faust : [docs/fad-note-en.md](../docs/fad-note-en.md),
  [docs/ondemand-note-fr.md](../docs/ondemand-note-fr.md),
  [docs/rad-note-en.md](../docs/rad-note-en.md),
  [docs/rad-usage-en.md](../docs/rad-usage-en.md),
  [docs/fad-rad-synthesis-fr.md](../docs/fad-rad-synthesis-fr.md),
  [docs/fad-debruijn-recursion-en.md](../docs/fad-debruijn-recursion-en.md).
