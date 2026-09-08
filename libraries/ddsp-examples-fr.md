# Douze exemples DDSP avec `fad` et `rad`

Douze programmes complets de DSP différentiable, chacun une tâche qu'un
ingénieur du son reconnaît, écrits avec les deux primitives de
différenciation automatique de `faust-rs` et les boucles
d'[optimizers.lib](optimizers.lib). Trois utilisent `fad`, le mode direct, là
où la dérivée exacte à travers une récursion est ce qui fait marcher la
méthode ; trois utilisent `rad`, le mode inverse, là où une perte scalaire
dépend de nombreux paramètres ou là où le gradient sort du graphe vers un
hôte. Trois autres, à la fin, sont l'état de l'art de leur domaine : un diode
clipper dont on apprend les composants à travers son solveur implicite, une
réverbération FDN calibrée sur une décroissance cible, un amplificateur
neuronal récurrent entraîné par rétropropagation dans le temps tronquée. Les
deux derniers sont les fragiles, gardés pour ce qu'ils enseignent : la hauteur
d'une corde apprise à travers son retard fractionnaire, et le synthétiseur
harmonique de DDSP ajusté par une perte spectrale calculée trame par trame
dans un bloc `ondemand`. Le douzième est de nouveau la réverbération FDN,
qui se calibre puis coupe son apprentissage, pour ne plus coûter qu'une
réverbération une fois fait.
Chaque programme vit dans `tests/corpus/ddsp_*.dsp`, est exécuté par la
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
Les colonnes sont les sorties du programme dans l'ordre que donne son
en-tête ; dans les statistiques, `dc` (la moyenne) est la lecture d'un
paramètre et `rms` celle d'un résidu. Chaque section ci-dessous donne la
commande de son programme et ce qu'elle affiche ; les commandes se lancent
depuis la racine du dépôt.

| | Programme | Tâche | Mode | Boucle et moteur | Résultat |
|---|---|---|---|---|---|
| 1 | `ddsp_fad_adaptive_notch` | supprimer un ronflement de fréquence inconnue | `fad` | `lsq_1D` + `nlms` | 1000,0 ± 0,2 Hz depuis 1400 Hz, résidu au plancher de bruit |
| 2 | `ddsp_fad_modal_resonator_lm` | calibrer un mode (fréquence, Q) | `fad` | `lm_2D` (Gauss-Newton) | (800,000, 25,001) depuis (600, 10) |
| 3 | `ddsp_fad_amp_model` | apprendre un ampli (drive, gain, tone) de bout en bout | `fad` | `descend_3D` + Adam | (3,98, 0,701, 0,800) pour (4, 0,7, 0,8) |
| 4 | `ddsp_rad_echo_canceller_64` | annuler un écho acoustique de 64 coefficients | `rad` | `lsq_N_rad` + `nlms` | écho résiduel sous 1e-9 (ERLE > 100 dB) |
| 5 | `ddsp_rad_mlp_waveshaper` | entraîner un petit réseau de neurones à un soft clipper | `rad` | `descend_N_rad` + Adam | résidu 46 dB sous la cible |
| 6 | `ddsp_rad_host_block_resonator` | gradients par bloc d'un résonateur pour un hôte | `rad`, public | Adam dans l'hôte (Rust) | gradient = différences finies à cinq chiffres, (−1,20000, 0,72000) retrouvé |
| 7 | `ddsp_fad_diode_clipper_newton` | apprendre les composants d'un diode clipper à travers son solveur implicite | `fad` dans `fad` | `lm_2D` | (τ, k) exacts en 8 000 échantillons ; dérivée déroulée = implicite à 2e-7 |
| 8 | `ddsp_fad_fdn_reverb_lm` | calibrer une réverbération FDN sur une décroissance cible | `fad` | `lm_2D` | (T60, amortissement) = (0,600, 0,300) depuis (0,3, 0) |
| 9 | `ddsp_rad_gru_amp_host` | entraîner un ampli GRU par BPTT tronquée par blocs | `rad`, public | Adam dans l'hôte (Rust) | gradients = différences finies à quatre chiffres ; résidu 29 dB sous la cible |
| 10 | `ddsp_fad_waveguide_string_pitch` | accorder une corde à guide d'onde à travers son retard fractionnaire | `fad` | `lsq_1D` + `nlms` | 228 → 220,000000 Hz ; puits de ±1 Hz, capture seulement par le haut |
| 11 | `ddsp_rad_harmonic_spectral_frame` | ajuster 16 amplitudes harmoniques par une perte spectrale par trame | `rad` dans `ondemand` | Adam par trame, dans un bloc `ondemand` | toutes les amplitudes à 2,5e-4 de 1/h en 100 trames |
| 12 | `ddsp_fad_fdn_gated` | calibrer la FDN, puis couper son apprentissage | `fad` dans `gated` | `lm_2D` dans un `ondemand` cadencé par `stop_below`, gains par `on_change` | (0,600, 0,300) figés à la sixième période ; l'apprentissage ne coûte plus rien |

**Où tourne l'optimiseur.** Huit exemples font un pas par échantillon audio
dans le graphe, par les boucles de la bibliothèque (`lsq_1D`, `lm_2D`,
`descend_3D`, `lsq_N_rad`, `descend_N_rad`) : 1, 2, 3, 4, 5, 7, 8 et 10.
L'exemple 11 est le seul dont l'optimiseur tourne dans un bloc `ondemand` :
sa perte est calculée une fois par trame de 256 échantillons dans le bloc et
Adam y fait son pas, à la cadence des trames, dans un bloc écrit à la main
autour de `frame_sum` et `adam_g`. Les boucles cadencées de la bibliothèque
(`descend_1D_clocked` … `descend_5D_clocked`, `descend_N_clocked`,
`descend_N_rad_clocked`) emballent l'autre motif cadencé, une perte calculée
à cadence audio et son gradient moyenné sur la trame, un pas par
déclenchement ; aucun des onze premiers ne les utilise, la section 11 du
tutoriel et les fixtures `opt_descend_clocked_gain.dsp` et
`opt_descend_in_ondemand_gain.dsp` le font. L'exemple 12 fait tourner la
boucle de l'exemple 8 à cadence audio dans `op.gated`, un bloc `ondemand`
que son propre drapeau arrête : le seul dont l'apprentissage se termine, et
celui qui utilise les fonctions de porte de la bibliothèque, `stop_below`
pour le drapeau et `on_change` pour les coefficients de la réverbération
rendue. Les exemples 6 et 9 n'utilisent
pas `ondemand` du tout : leur optimiseur est celui de l'hôte, un pas d'Adam
par bloc `compute` sur les voies de gradient sommées.

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

**Avec faustprobe.** La colonne 2 est la fréquence du zéro, la colonne 1 le
signal nettoyé :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 20000 --every 4000 tests/corpus/ddsp_fad_adaptive_notch.dsp
```

affiche 1400 à la trame 0, 999,9 à 4 000, 1000,05 à 8 000, puis à moins de
0,1 Hz de 1000. Ajouter `--quiet --skip 16000` pour les statistiques des
4 000 dernières trames : `out0` rms 0,0120, le plancher du bruit ajouté, et
`out1` dc 1000,00, la moyenne de la fréquence suivie.

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

**Avec faustprobe.** Colonnes f, q, résidu :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 20000 --every 4000 tests/corpus/ddsp_fad_modal_resonator_lm.dsp
```

f dépasse à 808,6 à 4 000 et vaut 799,9 à 8 000 ; q se referme plus
lentement, 22,4, 24,3, 24,7, 25,3 aux trames affichées, 25,0 en moyenne ; la
colonne du résidu reste à quelques 1e-3 au plus.

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

**Avec faustprobe.** Colonnes drive, gain, tone, résidu :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 20000 --every 4000 tests/corpus/ddsp_fad_amp_model.dsp
```

`(2,68, 0,80, 0,81)` à 4 000, `(4,000, 0,700, 0,800)` à 8 000 et 16 000. Une
trame affichée peut tomber sur la gigue d'Adam (4,14 à 12 000 dans un essai),
raison pour laquelle le test moyenne les 4 000 derniers échantillons :
`--quiet --skip 16000` donne cette moyenne dans la colonne `dc`.

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

**Avec faustprobe.** La colonne 1 est l'écho résiduel, la colonne 2 le
microphone :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 1000 --quiet tests/corpus/ddsp_rad_echo_canceller_64.dsp
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 12000 --skip 8000 --quiet tests/corpus/ddsp_rad_echo_canceller_64.dsp
```

Le premier essai montre le résidu au niveau de l'écho, rms 2,6 avec une
pointe transitoire à 25 ; le second, sur les trames 8 000 à 12 000, un rms de
`out0` nul à la précision affichée contre un rms de `out1` de 1,03 : l'ERLE
dépasse 100 dB sur cette pièce sans bruit.

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

**Avec faustprobe.** La colonne 1 est le résidu, la colonne 2 la cible :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 2000 --quiet tests/corpus/ddsp_rad_mlp_waveshaper.dsp
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 20000 --skip 16000 --quiet tests/corpus/ddsp_rad_mlp_waveshaper.dsp
```

rms 0,105 sur les 2 000 premières trames, 0,0037 sur les trames 16 000 à
20 000 contre une cible de rms 0,71 : 46 dB plus bas.

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

**Avec faustprobe.** Le programme a besoin d'une entrée et d'un hôte ;
`faustprobe` fournit l'entrée et montre les voies que l'hôte sommerait, mais
ne fait pas tourner la boucle d'Adam (c'est la part du test Rust) :

```sh
faustprobe --double -I libraries -I <faustlibraries> --list-params tests/corpus/ddsp_rad_host_block_resonator.dsp
faustprobe --double -I libraries -I <faustlibraries> --in white:1 -n 256 --quiet tests/corpus/ddsp_rad_host_block_resonator.dsp
faustprobe --double -I libraries -I <faustlibraries> --in white:1 -n 256 --quiet --set /ddsp_rad_host_block_resonator/a1=-1.2 --set /ddsp_rad_host_block_resonator/a2=0.72 tests/corpus/ddsp_rad_host_block_resonator.dsp
```

La première liste les deux sliders et leurs chemins. La deuxième, sur un
bloc de 256 trames de bruit blanc aux valeurs initiales `(−0,8, 0,5)`, donne
dans la colonne `dc` les contributions moyennes par échantillon, perte 0,46,
gradients 1,78 et 1,27 ; multipliées par 256, ce sont la perte et le gradient
de bloc sur lesquels un hôte ferait son pas. La troisième règle les sliders
sur la cible cachée `(−1,2, 0,72)` : les trois voies valent exactement 0.

**À essayer.** Remplacer la cible par un enregistrement et la perte par une
perte spectrale calculée par l'hôte : le DSP reste le même. Grouper
plusieurs excitations par mise à jour. Entraîner les cinq coefficients d'un
biquad (`rad(loss, (b0, b1, b2, a1, a2))`) : une voie de plus chacun, un seul
balayage.

## État de l'art : trois de plus

Les six programmes ci-dessus sont le manuel de l'audio adaptatif ; les trois
ci-dessous sont ce que fait la littérature du DSP différentiable de ces cinq
dernières années, et chacun repose sur quelque chose qu'un framework à
tenseurs ne donne pas : la dérivée exacte à travers un solveur implicite, à
travers des milliers d'échantillons de rétroaction, ou le balayage inverse à
travers une cellule récurrente sans réécrire le modèle.

## 7. Un diode clipper appris à travers son solveur implicite (`fad` dans `fad`)

**Ce que fait le programme.** Le circuit de toute pédale d'overdrive : une
résistance, un condensateur et une paire de diodes (Yeh, Abel & Smith 2007),
`dv/dt = (x − v)/(RC) − (2 Is/C) sinh(v/(2 n Vt))`. Discrétisé par Euler
implicite, c'est une équation implicite en v[n],
`G(v) = v − v[n−1] − h f(v, x[n]) = 0`, résolue à chaque échantillon par quatre
itérations de Newton sécurisées dont la pente `G'(v)` vient d'un `fad`
intérieur — un modèle analogique virtuel à rétroaction sans retard au sens
habituel. Deux valeurs de composants, τ = RC et k = 2 Is/C, sont ensuite
apprises sur la sortie d'un clipper caché : la modélisation analogique
virtuelle « boîte blanche » (Esqueda, Kuznetsov & Parker 2021), dans le fil
audio.

**Modèle.** Une excitation de type guitare (trois partiels et un bruit à
bande limitée, environ ±1,5 V, pour que les diodes conduisent sur les
crêtes) ; `h = 1/SR`, `2 n Vt = 0,09 V` ; cible `(τ, k) = (1e-4 s, 0,1)`,
soit 2,2 kΩ · 47 nF ; le modèle part de `(3e-4, 0,03)`, les deux en domaine
log. L'itération de Newton part d'un prédicteur d'Euler explicite et garde
son itéré dans ±2 V.

**Ce qui est dérivé, et pourquoi le mode direct.** `lm_2D` dérive la sortie
du clipper par rapport à `(log τ, log k)` : le `fad` extérieur traverse les
quatre pas de Newton déroulés — chacun contenant un `fad` intérieur pour la
pente — et la récursion d'état : `fad` dans `fad` dans une récursion, le tout
développé à la compilation. Le programme vérifie le résultat contre le
théorème des fonctions implicites : la dérivée du v résolu par rapport à k,
propagée à travers la récursion, `s[n] = −(G_k + G_vprev · s[n−1]) / G_v`,
coïncide avec la dérivée déroulée à 2e-7 près, dans les deux précisions,
tandis que le résidu de Newton reste sous 1e-8 (1,2e-7 en simple précision).
Mode direct : deux tangentes à travers un solveur dont le `fad` intérieur
fournit déjà la jacobienne. Deux choses devaient tenir pour que cela marche
en simple précision, et les deux sont maintenant dans le compilateur et dans
les pièges : une récursion que la graine n'atteint pas n'est pas augmentée
(toute la boucle `lm_2D` était copiée dans le `fad` intérieur, avec des
tangentes exactement nulles en théorie et `inf · 0` en `f32`), et
l'itération ne doit pas partir du signal même que l'équation tient fixe —
les graines sont reconnues par identité, `fad(G(vprev, v), v)` avec
`v = vprev` dérive les deux.

**Optimiseur.** `lm_2D(mdl, 0.01, 0.1, 0.99, …)` : Gauss-Newton amorti avec
la jacobienne exacte à travers le solveur.

**Ce qu'on observe.** `(τ, k) → (1,0000e-4, 0,1000)` en 8 000 échantillons,
le résidu par rapport au clipper caché à 1,7e-7 rms en simple précision.

**Avec faustprobe.** Colonnes τ × 1e4, k, résidu, résidu de Newton, écart
des dérivées, dérivée :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 20000 --every 4000 tests/corpus/ddsp_fad_diode_clipper_newton.dsp
```

τ × 1e4 passe de 3,0 à 1,000 et k de 0,03 à 0,1000 dès la ligne de la trame
4 000 ; le résidu, le résidu de Newton et l'écart entre les deux dérivées
s'affichent à 0 sur neuf décimales ; la dernière colonne, dv/dk lui-même,
croît avec le signal de 0,05 à 0,56, l'échelle à laquelle se lit l'écart.

**À essayer.** Apprendre aussi `2 n Vt` (`lm_3D`) ; un clipper asymétrique
(une diode, `exp` au lieu de `sinh`) ; un second étage RC ; fournir un
enregistrement et voir l'identifiabilité dépendre de la force avec laquelle
l'entrée pousse les diodes.

## 8. Une réverbération FDN calibrée sur une décroissance cible (`fad`)

**Ce que fait le programme.** Un réseau de retards à rétroaction à quatre
lignes (Jot 1991) : des retards premiers de 1051, 1327, 1597 et 1801
échantillons (24 à 41 ms), une matrice de Hadamard orthogonale (mise à
l'échelle par 1/2), un gain par ligne fixé par un temps de réverbération,
`gain_i = 10^(−3 len_i / (T60 · SR))`, et un amortissement à un pôle par
ligne qui raccourcit la décroissance des aigus. Étant données les réponses
d'un FDN caché à un train d'impulsions, le programme apprend son T60 et son
amortissement : la réverbération artificielle différentiable (Lee, Choi &
Lee 2022).

**Modèle.** Une impulsion tous les 16 384 échantillons ; cible
`(T60, d) = (0,6 s, 0,3)` ; départ `(0,3 s, 0)`, T60 en domaine log.

**Ce qui est dérivé, et pourquoi le mode direct.** `fad` transporte une
tangente à travers les quatre lignes à retard, les filtres d'amortissement
et la matrice de rétroaction, échantillon par échantillon : la dérivée d'une
queue de réverbération par rapport à ses paramètres de décroissance, exacte
à travers des récursions de milliers d'échantillons, là où un framework à
tenseurs déroule ou approche. Deux tangentes.

**Optimiseur.** `lm_2D` avec un facteur d'oubli de 0,999 : le gradient n'est
informatif que pendant les décroissances, et Gauss-Newton avec facteur
d'oubli garde la dernière décroissance dans sa matrice d'information. Adam
avec un schedule atteint aussi `(0,60, 0,30)`, puis erre entre les
impulsions quand le gradient ne porte plus d'information (le fixture le
dit).

**Ce qu'on observe.** `(0,574, 0,289)` après 8 000 échantillons,
`(0,6000, 0,3000)` à 60 000 (quatre impulsions), le résidu à 4,8e-7 rms à
80 000.

**Avec faustprobe.** Colonnes T60, amortissement, résidu ; `--every 16384`
affiche une ligne par impulsion :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 80000 --every 16384 tests/corpus/ddsp_fad_fdn_reverb_lm.dsp
```

T60 0,568 après la première période, 0,6002 après la deuxième, 0,6000 à
partir de la troisième ; amortissement 0,298, 0,2998, 0,29995, 0,30000 ; le
résidu descend à 3e-7.

**À essayer.** Apprendre un gain par ligne (`descend_N`) ; prendre pour cible
une réverbération *différente* et pour perte `log_energy_loss` sur la
décroissance ; huit lignes ; un T60 dépendant de la fréquence avec une cible
mesurée dans une salle.

## 9. Un ampli GRU entraîné par BPTT tronquée par blocs (`rad`, public)

**Ce que fait le programme.** Une cellule GRU à deux unités cachées et une
lecture linéaire, 27 paramètres — l'architecture de la modélisation
neuronale d'ampli en temps réel (Wright & Välimäki 2020) — est entraînée à
imiter un amplificateur caché (un contrôle de tonalité vers une saturation
`tanh`). Les paramètres sont des sliders ; le programme sort l'erreur
quadratique et ses 27 gradients, échantillon par échantillon ; l'hôte (le
test Rust) somme chaque voie sur le bloc et fait un pas d'Adam : la
rétropropagation dans le temps tronquée, avec le bloc pour longueur de
troncature.

**Modèle.** `z = σ(W_z x + U_z h + b_z)`, `r = σ(W_r x + U_r h + b_r)`,
`c = tanh(W_h x + U_h (r ∘ h) + b_h)`, `h' = (1 − z) ∘ h + z ∘ c`,
`y = W_o h' + b_o`, deux unités ; un slider par paramètre avec une valeur
initiale fixe (le parseur veut des libellés littéraux). Amplificateur caché
`0,8 · tanh(3 · si.smooth(0.7, x))`. `process = rad(loss, params)` : 28 voies.

**Ce qui est dérivé, et pourquoi le mode inverse.** Une perte, 27
paramètres : un balayage inverse. Parce que les voies sortent du graphe, le
balayage remonte tout le bloc à travers les portes, le candidat `tanh` et
les deux états rebouclés, avec un adjoint terminal nul à la fin du bloc : la
somme d'une voie est le gradient exact de la perte du bloc, état initial
tenu fixe — la BPTT tronquée au bloc, que le test vérifie contre des
différences finies centrées sur trois paramètres de natures différentes : un
poids d'entrée 0,3671 (0,3670), un poids récurrent 0,0288 (0,0288), un poids
de lecture −3,2342 (−3,2342). Consommé dans le graphe, le même `rad` ne
verrait qu'un échantillon, et un modèle récurrent ne s'entraîne pas
ainsi ; d'où l'hôte.

**Optimiseur.** Adam en Rust, `lr = 0,005` par bloc de 256 échantillons,
2 000 blocs (11,6 s d'audio), l'état conservé d'un bloc à l'autre.

**Ce qu'on observe.** La perte moyenne par bloc tombe de 4,7e-3 (100
premiers blocs) à 2,4e-4 (100 derniers) ; sur un bruit neuf, depuis une
instance neuve, le résidu vaut 0,0148 pour une cible de rms 0,43 : 29 dB
sous la cible.

**Avec faustprobe.** Comme pour l'exemple 6, `faustprobe` montre ce que
l'hôte lirait, pas l'entraînement :

```sh
faustprobe --double -I libraries -I <faustlibraries> --list-params tests/corpus/ddsp_rad_gru_amp_host.dsp
faustprobe --double -I libraries -I <faustlibraries> --in white:3 -n 256 --quiet tests/corpus/ddsp_rad_gru_amp_host.dsp
```

27 sliders ; sur un bloc de bruit blanc aux poids initiaux, `out0` a un `dc`
de 0,0119, la perte de bloc moyenne, et `out1` à `out27` les contributions
moyennes des gradients des 27 paramètres dans l'ordre de la liste `params` ;
l'hôte somme chacune sur le bloc et fait son pas.

**À essayer.** Quatre unités cachées (plus de sliders, même boucle hôte) ;
une cellule LSTM ; plusieurs excitations par mise à jour ; l'enregistrement
d'un vrai amplificateur comme modèle caché — le DSP ne change pas, seule la
cible de l'hôte.

## Les deux fragiles : la hauteur à travers un retard, le spectre à travers une trame

## 10. Une corde à guide d'onde apprend sa hauteur à travers un retard fractionnaire (`fad`)

**Ce que fait le programme.** Un modèle de corde pincée — une boucle avec un
retard fractionnaire (interpolation de Lagrange d'ordre 4), un gain de pertes
et un amortissement à un pôle — excité par du bruit ; la longueur du retard,
c'est-à-dire la hauteur, est apprise sur une corde cachée à 220 Hz par
moindres carrés normalisés sur la forme d'onde.

**Ce qui est dérivé, et pourquoi le mode direct.** `fad` dérive la sortie de la
boucle par rapport à la longueur du retard : à travers l'interpolation (la
dérivée d'une lecture interpolée par rapport à la position de lecture est la
pente locale du signal) et à travers la rétroaction, échantillon par
échantillon. Les frameworks à tenseurs n'ont pas de dérivée par rapport à une
longueur de retard ; ici elle coûte une tangente.

**Ce que permet le paysage.** L'erreur de forme d'onde entre deux cordes est un
puits de ±1 Hz de large autour de 220 Hz sur un plateau plat : résidu rms
0,10–0,11 de 150 à 300 Hz, 0,09 à ±1 Hz, 0 à 220. Sur le plateau le gradient
n'est pas nul : le retard de groupe du filtre de boucle décale le pic
d'autocorrélation de la corde par rapport à la longueur du retard, si bien que
la puissance de sortie du modèle lui-même dépend de `d` et que le pas
normalisé dérive vers une hauteur *plus basse* quelle que soit la cible (une
perte de corrélation, `−modèle · cible`, supprime ce biais mais n'attire pas
davantage sur le plateau). L'ajustement fin marche — depuis 228 Hz la hauteur
se cale à 220,000000 Hz en 60 000 échantillons, et depuis 264 Hz aussi quand
l'amortissement du modèle est recuit de 0,70 à 0,95 (résonances larges
d'abord) — et par le bas non (200 → 190 Hz, 176 → 168). C'est pourquoi les
systèmes DDSP estiment f0 par un détecteur et laissent le gradient affiner.

**Optimiseur.** `lsq_1D` avec `nlms(0.02, 1e-6, 0.99)`.

**Ce qu'on observe.** 228 → 219,99 Hz à 20 000 échantillons, 220,000000 à
60 000, résidu 3e-8.

**Avec faustprobe.** Colonnes hauteur en Hz, résidu :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 60000 --every 10000 tests/corpus/ddsp_fad_waveguide_string_pitch.dsp
```

228, 223,8, 219,99, 220,0007, 219,99999, 220,0000004 aux trames affichées ;
le résidu passe de 0,08 à 2e-7.

**À essayer.** Apprendre aussi l'amortissement (`lsq_2D`) ; remplacer le bruit
par des pincements et voir le puits se rétrécir ; partir une quinte plus loin
et voir la dérive ; donner l'estimation d'un détecteur de hauteur comme `init`.

## 11. Un synthétiseur harmonique ajusté par une perte spectrale par trame (`rad` dans un bloc `ondemand`)

**Ce que fait le programme.** Le banc d'oscillateurs harmoniques de DDSP
(Engel et al. 2020) : seize harmoniques de 440 Hz dont les amplitudes sont
apprises, positives par construction (`a_h = exp(p_h)`), ajustées à un signal
cible par une perte spectrale calculée une fois par trame de 256 échantillons.
La cible est ici un son harmonique caché d'amplitudes 1/h, mais tout audio
conviendrait : elle est analysée à cadence audio — corrélations fenêtrées aux
seize harmoniques, accumulées sur la trame avec `frame_sum` — et entre dans le
bloc par ses entrées.

**Ce qui est dérivé, pourquoi le mode inverse, et pourquoi un bloc.** Dans le
bloc, tiré une fois par trame, la trame du synthétiseur est calculée à partir
des log-amplitudes et du début de trame, ses magnitudes aux harmoniques sont
comparées à celles de la cible, et `rad` sur cette perte de trame donne les
seize gradients en un balayage inverse — dans le domaine propre du bloc, à la
cadence des trames, sur une perte sans récursion ; un pas d'Adam par trame. La
perte sur les magnitudes est aveugle au signe d'une amplitude (une harmonique
converge vers −a aussi volontiers que vers a), d'où les exponentielles, comme
dans DDSP. Trois choses devaient tenir dans le compilateur et la
bibliothèque : le balayage inverse traite les entrées de frontière d'un bloc
et les constantes étrangères (`ma.SR`) comme des feuilles et traverse
l'enveloppe d'horloge, si bien qu'un `rad` peut vivre *dans* un bloc (à
travers la frontière il reste refusé) ; et l'état d'une boucle est mieux
tenu comme un écart à `init`, sans aucune détection du premier échantillon,
ce que fait `optimizers.lib` 0.7.1.

**Ce qu'on observe.** Les seize amplitudes à 2,5e-4 (relatif) de 1/h en 100
trames, 0,6 s d'audio ; le résidu de resynthèse vaut 1,2e-3 rms pour une cible
de rms 0,8. Le graphe de trame — 256 × 16 sinus, 32 corrélations de 256
termes — se normalise en une seconde en build release et en deux minutes en
build non optimisé (la factorisation des termes additifs que fait aussi le
Faust C++), son test tourne donc sous `cargo test --release`.

**Avec faustprobe.** Les colonnes 1 à 16 sont les amplitudes, tenues
entre les trames, la colonne 17 le résidu ; les statistiques des 4 200
dernières trames sont la lecture :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 51200 --skip 47000 --quiet tests/corpus/ddsp_rad_harmonic_spectral_frame.dsp
```

`out0` dc 0,9998 (1/1), `out1` 0,4999 (1/2), ..., `out15` 0,06250 (1/16) ;
`out16` rms 4e-4. L'essai prend une quinzaine de secondes : la normalisation
du graphe de trame, une somme de 256 produits de sommes à 16 termes, domine.

**À essayer.** Un enregistrement comme cible (`--in`) ; plus d'harmoniques ;
l'autre moitié de DDSP, une bande de bruit à travers un filtre appris ; une
perte multi-résolution (deux tailles de trame, deux blocs).

## 12. Une réverbération qui se calibre, puis cesse de payer son apprentissage (`gated`, `stop_below`, `on_change`)

**Ce que ça fait.** La FDN de l'exemple 8, la même cible cachée, la même
boucle de Gauss-Newton, avec deux ajouts venus de la section « Gating and
Stopping » de la bibliothèque. Tout l'apprentissage, le modèle qui porte les
deux tangentes, `lm_2D` et le résidu, vit dans `op.gated(learn)`, un domaine
`ondemand` dont l'horloge est coupée par le propre drapeau du bloc : une
fois le drapeau levé, plus rien de l'apprentissage n'est calculé et les
paramètres tiennent. Et la réverbération qui rend la sortie prend ses
quatre gains d'`op.on_change`, qui ne recalcule `10^(−3 len_i / (T60 · SR))`
que lorsque T60 change, une fois par pas d'apprentissage et plus jamais
après l'arrêt, au lieu de quatre `pow` par échantillon.

**Le drapeau.** `op.stop_below(clock, 1e-7)` sur le résidu au carré : levé à
la fin de la première période dont l'énergie résiduelle est sous 1e-7, un
résidu de 2,5e-6 rms, une correspondance exacte pour un effet, et tenu
levé. Un seuil plutôt que `stop_relative` parce que la cible est exacte : le
résidu n'a pas de plancher, il continue de baisser géométriquement et son
changement relatif ne se stabilise jamais. Sur une cible mesurée, avec un
plancher de bruit, `stop_relative` est le critère ; la calibration d'une
réverbération sur des salles mesurées, section 7 de l'aperçu, l'utilise.
Gauss-Newton avec un facteur d'oubli de 0,999 est aussi la raison pour
laquelle la cible est exacte ici : un bruit à −60 dB suffit à faire errer
ses pas (essayez).

**Ce qu'on voit.** `(0,5688, 0,2980)` à la fin de la première période,
`(0,6000, 0,3000)` à la quatrième ; l'énergie résiduelle par période tombe
de 3,9e-2 à 1,6e-8 à la sixième, où le drapeau se lève au dernier
échantillon de la période (98 303) et les paramètres se figent à
`(0,600002, 0,300000)` ; la réverbération rendue sur les gains tenus
coïncide alors avec la cible à 5e-9 rms. Jusqu'au drapeau, un bloc cadencé
est bit-identique au même bloc hors de la porte (les fixtures de la
bibliothèque le vérifient) ; après, l'apprentissage ne coûte rien et la
réverbération coûte une réverbération.

**Avec faustprobe.** Colonnes T60, amortissement, done, résidu de la
réverbération rendue ; une ligne par période :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 160000 --every 16384 tests/corpus/ddsp_fad_fdn_gated.dsp
```

T60 et amortissement suivent l'exemple 8 ligne pour ligne ; `done` vaut 0
pendant cinq périodes et 1 à partir de la trame 98 304, la sixième période ;
dès cette ligne T60 affiche 0,600001606 sur toutes les lignes suivantes, au
bit près, et le résidu vaut 1e-9. Plus rien de l'apprentissage ne tourne.

**À essayer.** `gated_when(button("learn"), learn)` pour réapprendre à la
demande ; une cible qui change toutes les cent périodes, avec `gated_when`
qui réactive l'apprentissage quand l'énergie résiduelle remonte ;
`stop_after(clock, 8)` comme simple budget.

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
vérification par différences finies ; le diode clipper à 1 % près sur τ et k
avec un résidu de Newton sous 1e-4 et les deux dérivées à 1e-3 l'une de
l'autre, le FDN à 0,01 près sur T60 et l'amortissement, les gradients du GRU
à 2 % des différences finies, sa perte divisée par dix et son résidu 20 dB
sous la cible ; la corde à 0,05 Hz de 220 avec un résidu sous 1e-3 ; les
amplitudes harmoniques à 2 % de 1/h avec un résidu de resynthèse sous 0,01
(en build release) ; la FDN cadencée à 0,01 du T60 et de l'amortissement
quand son drapeau se lève, à une frontière de période entre la quatrième et
la vingtième, ses paramètres bit-constants ensuite et le résidu rendu sous
1e-4 rms. Les programmes tournent en simple précision là et en
double sous `faustprobe` ; les deux convergent.

## D'où viennent les gradients

`fad` est développé pendant la propagation en la récursion à état augmenté
décrite dans [docs/fad-note-en.md](../docs/fad-note-en.md) ; `rad` en le
balayage inverse par bloc de [docs/rad-note-en.md](../docs/rad-note-en.md),
dont les carries, les bandes et les horizons sont ce que les exemples 4 à 6
exercent. Les boucles à bus et les moteurs sont documentés fonction par
fonction dans [optimizers.lib](optimizers.lib).
