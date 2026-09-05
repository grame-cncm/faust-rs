# Six exemples DDSP avec `fad` et `rad`

Six programmes complets de DSP différentiable, chacun une tâche qu'un
ingénieur du son reconnaît, écrits avec les deux primitives de
différenciation automatique de `faust-rs` et les boucles
d'[optimizers.lib](optimizers.lib). Trois utilisent `fad`, le mode direct, là
où la dérivée exacte à travers une récursion est ce qui fait marcher la
méthode ; trois utilisent `rad`, le mode inverse, là où une perte scalaire
dépend de nombreux paramètres ou là où le gradient sort du graphe vers un
hôte. Chaque programme vit dans `tests/corpus/ddsp_*.dsp`, est exécuté par la
suite de tests
([crates/compiler/tests/ddsp_examples.rs](../crates/compiler/tests/ddsp_examples.rs))
et s'observe avec `faustprobe` :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 40000 --every 5000 tests/corpus/ddsp_fad_adaptive_notch.dsp
```

`-n` rend autant de trames, `--every` en affiche une sur N, `--quiet`
n'affiche que les statistiques, `--skip N` en exclut les N premières trames.
Ce document dit ce que fait chaque programme, ce qui est dérivé et pourquoi
dans ce mode, quel optimiseur il utilise et pourquoi, et ce que valent les
nombres. Le vocabulaire est dans
[optimizers-overview-fr.md](optimizers-overview-fr.md) ; l'introduction pas à
pas est [optimizers-ddsp-tutorial-fr.md](optimizers-ddsp-tutorial-fr.md).

| | Programme | Tâche | Mode | Boucle et moteur | Résultat |
|---|---|---|---|---|---|
| 1 | `ddsp_fad_adaptive_notch` | supprimer un ronflement de fréquence inconnue | `fad` | `lsq_1D` + `nlms` | 1000,0 ± 0,2 Hz depuis 1400 Hz, résidu au plancher de bruit |
| 2 | `ddsp_fad_modal_resonator_lm` | calibrer un mode (fréquence, Q) | `fad` | `lm_2D` (Gauss-Newton) | (800,000, 25,001) depuis (600, 10) |
| 3 | `ddsp_fad_amp_model` | apprendre un ampli (drive, gain, tone) de bout en bout | `fad` | `descend_3D` + Adam | (3,98, 0,701, 0,800) pour (4, 0,7, 0,8) |
| 4 | `ddsp_rad_echo_canceller_64` | annuler un écho acoustique de 64 coefficients | `rad` | `lsq_N_rad` + `nlms` | écho résiduel sous 1e-9 (ERLE > 100 dB) |
| 5 | `ddsp_rad_mlp_waveshaper` | entraîner un petit réseau de neurones à un soft clipper | `rad` | `descend_N_rad` + Adam | résidu 46 dB sous la cible |
| 6 | `ddsp_rad_host_block_resonator` | gradients par bloc d'un résonateur pour un hôte | `rad`, public | Adam dans l'hôte (Rust) | gradient = différences finies à cinq chiffres, (−1,20000, 0,72000) retrouvé |

## 1. Suppression d'un ronflement par notch adaptatif (`fad`)

**Ce que fait le programme.** L'entrée est un ronflement à 1 kHz (une
sinusoïde d'amplitude 0,5) dans un peu de bruit. Un filtre notch retire une
fréquence ; le programme apprend laquelle en minimisant la puissance de sa
propre sortie. C'est le notch adaptatif de Rao & Kung (1984) et Nehorai
(1985), la manière standard de suivre et de retirer une raie parasite sans
connaître sa fréquence.

**Modèle.** Le notch est contraint par construction : zéros sur le cercle
unité en ±w, pôles au rayon r = 0,95 juste derrière,

```text
H(z) = (1 − 2c z⁻¹ + z⁻²) / (1 − 2rc z⁻¹ + r² z⁻²),   c = cos w.
```

Le paramètre appris est c, si bien que toute valeur de [−1, 1] est un notch
valide ; la fréquence en hertz se relit avec `acos`. `r` fixe la largeur du
creux : un notch plus étroit (r plus proche de 1) atténue moins autour du
ronflement mais a un bassin d'attraction plus étroit.

**Ce qui est dérivé, et pourquoi le mode direct.** La boucle est `lsq_1D`
avec le notch pour modèle et une cible nulle : à chaque échantillon, `fad`
renvoie la sortie du notch et sa sensibilité `j = d(sortie)/dc`. Le notch
est récursif, donc `j` à l'échantillon n dépend de tout le passé du filtre ;
`fad` transporte cette dérivée avec l'état du filtre (la dérivée RTRL), qui
est exactement la quantité que les dérivations classiques approchent par un
« gradient simplifié ». Un paramètre, une tangente : le mode direct coûte un
filtre de plus.

**Optimiseur.** `nlms(0.002, 1e-6, 0.99)` : le pas `mu · r · j / E[j²]` est
proportionnel au résidu, il s'éteint donc de lui-même une fois le creux sur
le ronflement. Un pas d'Adam, normalisé à environ `lr` par échantillon,
continue sa marche aléatoire à l'optimum : le même programme avec
`descend_1D` et Adam vibre de ±25 Hz.

**Ce qu'on observe.** Depuis 1400 Hz : 966 Hz après 1 000 échantillons,
995,7 après 2 000, 999,9 après 4 000, puis 1000,0 ± 0,2 Hz. Le résidu tombe
du niveau du ronflement (rms 0,35) à rms 0,0118, le plancher du bruit ajouté
(0,02 uniforme : rms 0,0115) : le ronflement a disparu, le bruit est
intact.

**À essayer.** Déplacer `f0` pendant l'exécution (c'est une constante ici ;
en faire un slider) : le notch suit. Baisser `r` à 0,9 pour élargir la zone
de capture, le monter à 0,99 pour entendre à quel point un creux peut être
étroit. Remplacer la sinusoïde par deux sinusoïdes : un notch en suit une ;
deux notchs en cascade à deux paramètres (`lsq_2D`) suivent les deux.

## 2. Calibrer un mode par Gauss-Newton (`fad`)

**Ce que fait le programme.** Un mode d'un synthétiseur modal est un
passe-bande résonant avec une fréquence et un facteur de qualité (sa
décroissance). Étant donnée la réponse d'un mode caché à du bruit, le
programme identifie les deux paramètres d'un mode modèle. Calibrer des modes
à partir d'enregistrements est le pain quotidien du DDSP par modèles
physiques ; en voici un mode, avec la méthode du second ordre.

**Modèle.** `fi.resonbp(f, q, 1)` avec f en hertz et q sans unité ; la cible
est `(800, 25)`, le modèle part de `(600, 10)`.

**Ce qui est dérivé, et pourquoi le mode direct.** `lm_2D` dérive le
*modèle* par rapport à ses deux paramètres : à chaque échantillon, `fad`
donne les deux sensibilités de la sortie du résonateur, exactes à travers sa
récursion, et la boucle résout les équations normales 2×2 construites avec
elles (un pas de Gauss-Newton amorti, Levenberg-Marquardt), avec un facteur
d'oubli de 0,99 et un amortissement de Marquardt de 0,1. Deux paramètres
d'unités incompatibles — des hertz et un Q — font des pas de la bonne
échelle sans aucun réglage ; un moteur du premier ordre aurait besoin d'un
domaine log ou de vitesses par paramètre (tutoriel, section 5). Le mode
direct est la manière naturelle d'obtenir une ligne de jacobienne par
échantillon : deux tangentes.

**Ce qu'on observe.** f atteint 799,87 Hz en 8 000 échantillons et q 24,3,
tous deux exacts (800,000, 25,001) à 24 000 ; le résidu tombe à rms 2,6e-5.
Q touche brièvement sa borne haute (60) en chemin : le pas amorti est
audacieux tant que la jacobienne est petite, et c'est la borne qui le tient.

**À essayer.** Exciter par un train d'impulsions plutôt que du bruit (la
calibration n'apprend alors que pendant les décroissances). Ajouter un
troisième paramètre, le gain du mode, avec `lm_3D`. Deux modes : deux
boucles `lm_2D` sur la même cible ne peuvent pas les séparer ; un
`descend_5D` à cinq paramètres sur la somme le peut.

## 3. Un modèle d'amplificateur appris de bout en bout (`fad`)

**Ce que fait le programme.** Le plus petit « ampli » : un drive vers une
saturation `tanh`, un contrôle de tonalité (un passe-bas à un pôle), un
gain. Étant donnée la sortie d'un ampli caché sur du bruit, les trois
paramètres sont appris sur l'erreur de forme d'onde. C'est la forme de toute
tâche de modélisation neuronale d'ampli, réduite à un modèle à trois boutons
interprétables.

**Modèle.** `amp(ldrive, gain, tone, x) = gain · tanh(e^ldrive · x) : si.smooth(tone)`.
Le drive est appris dans le domaine log (un paramètre multiplicatif, dont la
plage utile couvre une décade), la tonalité comme coefficient du pôle borné
dans [0, 0,95] (un filtre stable par construction), le gain dans [0, 2].
Cible `(4, 0,7, 0,8)`, départ `(1, 1, 0,5)`.

**Ce qui est dérivé, et pourquoi le mode direct.** `descend_3D` dérive la
perte `mse(amp(p, x), cible)` par rapport aux trois paramètres. `fad`
traverse le `tanh` étranger (la `ffunction` de `maths.lib`) et la récursion
du un-pôle : la dérivée de la sortie par rapport au coefficient du pôle
dépend de tout le passé du filtre, et `fad` la transporte exactement. Trois
tangentes à travers un petit modèle : le mode direct est bon marché ici, et
il est consommé immédiatement dans le graphe.

**Optimiseur.** Un `adam_g(0.002, 0.9, 0.999, 1e-8)` par paramètre : le
drive, le gain et la tonalité ont des sensibilités différentes, et Adam
normalise chaque pas séparément. Adam garde une petite gigue à l'optimum (la
moyenne du drive sur les 4 000 derniers échantillons est 3,98, sa pointe
4,27) ; le test lit les moyennes. Un schedule (`lr_exp`) ou la moyenne
`polyak` retirent la gigue pour un modèle déployé.

**Ce qu'on observe.** `(4,00, 0,700, 0,800)` à 8 000 échantillons, un résidu
rms de 4e-3 sur une cible d'amplitude 0,8.

**À essayer.** Remplacer le bruit par une excitation de type guitare (une
somme de dents de scie décroissantes) et voir l'identifiabilité s'en aller :
le drive ne s'apprend que là où le signal sature. Apprendre un second étage
(un `tanh` après la tonalité) avec `descend_5D`. Remplacer `tanh` par un
waveshaper à table : `fad` dérive les tables en lecture seule par différences
finies sur l'index.

## 4. Un annuleur d'écho acoustique à 64 coefficients (`rad`)

**Ce que fait le programme.** Le signal distant part dans un haut-parleur ;
le microphone capte son écho à travers la pièce. L'annuleur apprend une
réplique FIR de la réponse de la pièce et la soustrait du signal du
microphone — l'annuleur d'écho NLMS de tout système de conférence (Haykin,
*Adaptive Filter Theory*). La pièce est ici une réponse synthétique à 64
coefficients, `h_i = sin(1,7 i + 0,3) · e^(−i/12)`.

**Modèle.** `fir`, un bloc dont les 64 premières entrées sont les
coefficients et la dernière le signal distant, appliqué aux échantillons
distants retardés ; `lsq_N_rad(64, fir, nlms(0.01, 1e-6, 0.99), −2, 2, 0, 0, mic, far)`.

**Ce qui est dérivé, et pourquoi le mode inverse.** La sensibilité de la
sortie du FIR au coefficient i est l'échantillon distant retardé x[n−i] : 64
sensibilités, une sortie. Le mode inverse les donne toutes en un balayage
par échantillon, là où `lsq_N` transporterait 64 tangentes — sur un FIR à 16
coefficients la boucle inverse compile en 3× moins d'instructions
d'interpréteur, à 64 coefficients 7× (synthèse, section 5). Le corps est
sans récursion vis-à-vis des coefficients, donc l'horizon d'un échantillon
d'un `rad` dans le graphe ne perd rien : le gradient est exact.

**Optimiseur.** `nlms` par coefficient (la bibliothèque normalise chaque
coefficient par la puissance de sa propre sensibilité), `mu = 0,01` : avec
64 coefficients qui partagent le pas, c'est la borne de stabilité du NLMS
classique (`mu < 2/N` dans ces unités) qui le fixe.

**Ce qu'on observe.** Le résidu part au niveau de l'écho (rms 1,8 sur les
2 000 premiers échantillons, avec une pointe transitoire à 25 pendant que
les coefficients dépassent) et passe sous 1e-9 à 8 000 échantillons : un
rehaussement de l'affaiblissement d'écho (ERLE) au-delà de 100 dB sur cette
pièce sans bruit. Ajouter du bruit côté proche et le résidu se cale à son
niveau.

**À essayer.** Changer la pièce en cours d'exécution (faire dépendre la
réponse d'un slider) : l'annuleur reconverge. Ajouter un locuteur proche :
le problème classique de la double parole — les coefficients dérivent ;
conditionner la mise à jour avec `gate_g` sur un détecteur de double parole.
Comparer avec `lsq_N` (mode direct) : même résidu, sept fois le code.

## 5. Un petit réseau de neurones apprend un waveshaper (`rad`)

**Ce que fait le programme.** Un réseau à une couche cachée de quatre
unités `tanh` (13 paramètres) est entraîné dans le graphe à imiter un soft
clipper, `0,8 · tanh(3x) + 0,1x`. C'est la modélisation neuronale d'ampli au
plus petit : une perte scalaire, un réseau, une descente de gradient sur
l'erreur de forme d'onde.

**Modèle.** `net`, un bloc de ses 13 paramètres `(w1 × 4, b1 × 4, w2 × 4, b2)` :
`y = Σ_j w2_j · tanh((w1_j + w1⁰_j) x + b1_j + b1⁰_j) + b2`. Une boucle à bus
démarre tous les paramètres à la même valeur, ce qui laisserait les quatre
unités cachées identiques à jamais ; le modèle ajoute des décalages fixes et
distincts `w1⁰_j = 1 + 0,5 j`, `b1⁰_j = −0,6 + 0,4 j` aux poids appris, si
bien que les paramètres sont appris depuis zéro autour d'une initialisation
déterministe.

**Ce qui est dérivé, et pourquoi le mode inverse.** `descend_N_rad(13,
net_loss, adam_g(0.003, 0.9, 0.999, 1e-8), −4, 4, 0, 0)` : la perte
`mse(net(p), cible)` est dérivée par un balayage inverse par échantillon
pour les 13 gradients. Le mode inverse *est* la rétropropagation : une perte
scalaire, beaucoup de paramètres, l'adjoint qui remonte de la couche de
sortie vers chaque unité. Le réseau est sans récursion, donc le balayage
dans le graphe est exact.

**Optimiseur.** Adam, partagé par les 13 paramètres (une expression de
moteur, un état par paramètre) : les unités ont des sensibilités
différentes et Adam les égalise.

**Ce qu'on observe.** Le résidu tombe de rms 0,105 sur les 2 000 premiers
échantillons (la fonction initiale des décalages est 17 dB sous la cible) à
0,0034 sur les 4 000 derniers : 46 dB sous la cible, une amélioration de
30 dB.

**À essayer.** Plus d'unités (`H = 8`) : la boucle à bus n'a besoin que de la
constante. Une cible plus difficile, avec mémoire — un un-pôle après le
clipper — et le réseau ne peut pas suivre (il n'a pas d'état) : ajouter un
un-pôle appris après `net`, ou donner `x` et `x'` aux unités. Retirer les
décalages : partir de `w1⁰` nul et voir les unités s'effondrer l'une sur
l'autre.

## 6. Gradients par bloc d'un résonateur, remis à un hôte (`rad`, public)

**Ce que fait le programme.** Les deux coefficients du dénominateur d'un
filtre résonant sont des sliders. Le programme sort, échantillon par
échantillon, l'erreur quadratique par rapport à un résonateur caché et les
deux gradients de cette erreur par rapport aux sliders — et n'apprend rien
lui-même. L'hôte (le test Rust, un plugin, un script Python) somme les
voies de gradient sur chaque bloc et met à jour les sliders avec Adam.
C'est le motif piloté par l'hôte de
[docs/rad-usage-en.md](../docs/rad-usage-en.md), sur un modèle récursif.

**Modèle.** `resonator(c1, c2, x) = fi.tf2(1, 0, 0, c1, c2, x)` ; cible
`(−1,2, 0,72)` (pôles au rayon 0,85, à 45°), sliders partant de
`(−0,8, 0,5)`. `process = rad(loss, (a1, a2))` avec
`loss = (cible − modèle)²` : trois sorties, `[loss, ∂loss/∂a1, ∂loss/∂a2]`.

**Ce qui est dérivé, et pourquoi le mode inverse.** Parce que les voies de
gradient sortent du graphe, le balayage inverse remonte tout le bloc
`compute()` : l'adjoint de l'état du résonateur est transporté d'échantillon
en échantillon dans le bloc (adjoint terminal nul à la fin du bloc), si bien
que la somme d'une voie sur le bloc est le gradient exact de la perte du
bloc. L'hôte peut le vérifier par différences finies, et le test le fait :
en `(−0,8, 0,5)` sur un bloc de 256, les voies sommées valent 299,609 et
198,821 là où les différences centrées sur les sliders donnent 299,605 et
198,821 (interpréteur en simple précision, `h = 1e-3`). Consommé dans le
graphe, le même `rad` ne verrait qu'un échantillon et renverrait le terme
direct (synthèse, section 4.7) ; cet exemple est celui dont le gradient est
exact à travers la récursion *et* vient d'un balayage inverse — au prix de la
boucle hôte.

**Optimiseur.** Adam en Rust, `lr = 0,01` par bloc de 256 échantillons, avec
correction de biais, les pôles maintenus dans le triangle de stabilité
(`|a2| < 1`, `|a1| < 1 + a2`). Les sliders sont écrits par leurs offsets
dans le tas (`set_real_zone`), l'excitation est le bruit LCG du corpus.

**Ce qu'on observe.** En 600 blocs (3,5 s d'audio) les sliders atteignent
`(−1,20000, 0,72000)` et la perte moyenne par bloc tombe de 0,53 à 2,6e-14.

**À essayer.** Remplacer la cible par un enregistrement et la perte par une
perte spectrale calculée par l'hôte : le DSP reste le même. Grouper
plusieurs excitations par mise à jour. Entraîner les cinq coefficients d'un
biquad (`rad(loss, (b0, b1, b2, a1, a2))`) : une voie de plus chacun, un seul
balayage.

## Comment les tests les vérifient

Chaque programme est rendu par l'interpréteur sur une instance neuve (les
bibliothèques standard sont trouvées par `FAUST_RS_FAUSTLIBRARIES_ROOT` ou le
chemin par défaut ; les tests sont sautés en leur absence), et les
vérifications sont les nombres ci-dessus avec une marge : le notch à 0,5 Hz
près et le résidu sous 0,02 rms, le mode à 0,5 Hz et 0,1 en Q près, l'ampli à
2 % près sur les moyennes des 4 000 derniers échantillons, l'annuleur d'écho
au-dessus de 30 dB d'ERLE, le réseau 20 dB sous la cible avec une
amélioration d'un facteur cinq par rapport à son départ, la boucle hôte à
0,02 près de la cible avec une réduction de 30 dB de la perte après la
vérification par différences finies. Les programmes tournent en simple
précision là et en double sous `faustprobe` ; les deux convergent.

## D'où viennent les gradients

`fad` est développé pendant la propagation en la récursion à état augmenté
décrite dans [docs/fad-note-en.md](../docs/fad-note-en.md) ; `rad` en le
balayage inverse par bloc de [docs/rad-note-en.md](../docs/rad-note-en.md),
dont les carries, les bandes et les horizons sont ce que les exemples 4 à 6
exercent. Les boucles à bus et les moteurs sont documentés fonction par
fonction dans [optimizers.lib](optimizers.lib).
