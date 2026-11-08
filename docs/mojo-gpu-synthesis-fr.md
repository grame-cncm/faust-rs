---
title: "Faust sur le GPU avec le backend Mojo : une synthèse accessible"
date: 2026-11-08
---

**Date :** 2026-11-08

**Public :** un lecteur curieux, un utilisateur de Faust ou un musicien qui se
demande ce qu'une carte graphique peut faire pour le son. Aucune connaissance
des GPU ni des compilateurs n'est supposée.

Version anglaise : [mojo-gpu-synthesis-en.md](mojo-gpu-synthesis-en.md) (même
contenu ; garder les deux versions synchronisées).

**Documents de référence :**

- [`faust-gpu-audio-synthesis-2026-11-07-en.md`](../porting/faust-gpu-audio-synthesis-2026-11-07-en.md),
  les mesures et le verdict détaillé ;
- [`mojo-backend-port-analysis-and-plan-2026-11-06-en.md`](../porting/mojo-backend-port-analysis-and-plan-2026-11-06-en.md),
  le backend Mojo ;
- [`mojo-gpu-batch-of-voices-plan-2026-11-07-en.md`](../porting/mojo-gpu-batch-of-voices-plan-2026-11-07-en.md)
  et
  [`mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md`](../porting/mojo-gpu-batch-g3-voice-layout-design-2026-11-07-en.md),
  le travail sur le GPU ;
- [`batch-state-register-promotion-plan-2026-11-08-en.md`](../porting/batch-state-register-promotion-plan-2026-11-08-en.md),
  l'état des copies gardé dans des variables locales (§5) ;
- DIFF-BACK-010 dans
  [`faust-rs-vs-faust-cpp-differences-en.md`](../porting/faust-rs-vs-faust-cpp-differences-en.md),
  ce qui diffère de Faust C++.

**Objectif :** expliquer ce que le backend Mojo rend possible (exécuter
certains programmes Faust sur un GPU), la puissance de calcul que cela
apporte par rapport au processeur, et où l'approche s'arrête.

**Sommaire**

1. [De quoi s'agit-il ?](#1-de-quoi-sagit-il-) — un nouveau langage de sortie pour Faust.
2. [Un CPU et un GPU](#2-un-cpu-et-un-gpu) — quelques cuisiniers rapides ou mille cuisiniers lents.
3. [Pourquoi un programme Faust seul ne va pas plus vite](#3-pourquoi-un-programme-faust-seul-ne-va-pas-plus-vite) — l'ordre du temps dans les récursions.
4. [L'idée qui marche : beaucoup de copies du même programme](#4-lidée-qui-marche--beaucoup-de-copies-du-même-programme) — voix, partiels, modes.
5. [Ce que fait le compilateur](#5-ce-que-fait-le-compilateur) — la mémoire rangée pour le GPU.
6. [Ce qui a été mesuré](#6-ce-qui-a-été-mesuré) — d'un GPU plus lent à un programme que seul le GPU peut jouer, puis un apprentissage : retrouver les modes d'un son.
7. [CPU et GPU dans le même programme](#7-cpu-et-gpu-dans-le-même-programme) — le chœur dans une salle.
8. [Latence : le plus petit buffer audio](#8-latence--le-plus-petit-buffer-audio) — pourquoi le GPU demande des blocs plus grands que le CPU, et comment PortAudio est piloté.
9. [Essayer](#9-essayer) — installer la chaîne d'outils, les commandes, et écrire son propre programme GPU.
10. [Les limites actuelles](#10-les-limites-actuelles) — ce que l'approche ne fait pas, ou pas encore.
11. [À retenir](#11-à-retenir) — la carte en une page.

## 1. De quoi s'agit-il ?

Faust décrit un traitement du son (un oscillateur, un filtre, une
réverbération, un synthétiseur entier) sous forme de formule. Le compilateur
transforme cette formule en code dans un autre langage : C, C++, Rust,
WebAssembly, et d'autres.

`faust-rs` sait maintenant écrire aussi du **Mojo** : `faust-rs -lang mojo
foo.dsp`. Mojo est un langage récent, proche de Python par sa syntaxe,
compilé en code machine rapide, et conçu pour exécuter le même code sur un
processeur (CPU) ou sur une carte graphique (GPU). Le backend suit le contrat
d'un backend Mojo proposé pour Faust C++ (la pull request
grame-cncm/faust#1293), si bien que le code qu'il écrit se branche sur les
mêmes fichiers d'architecture Mojo.

Sur un CPU, un programme Faust en Mojo calcule la même chose qu'en C++ : le
backend passe les 132 programmes de la suite de tests de réponse
impulsionnelle. La nouveauté, c'est que le même code généré peut tourner
**sur le GPU**, à condition de demander au GPU la bonne chose.

## 2. Un CPU et un GPU

> **Définition : CPU et GPU.** Un **CPU** a quelques cœurs (10 sur la machine mesurée ici, un Apple M5),
> chacun très rapide et très polyvalent. Un **GPU** a des milliers de petites
> unités de calcul, chacune plus lente, groupées pour exécuter la même
> instruction au même moment sur des données différentes.

Une image aide. Un CPU, c'est une cuisine avec quelques excellents
cuisiniers : chacun peut préparer n'importe quel plat, vite, l'un après
l'autre. Un GPU, c'est une cantine de mille cuisiniers moins habiles, mais qui
suivent tous la même recette au même moment. Demandez à la cantine un seul
plat gastronomique, et un seul cuisinier travaille, lentement. Demandez-lui
mille plats identiques, et elle est imbattable.

Les mesures disent exactement cela. Une réverbération Faust sur le GPU tourne
**30 à 100 fois plus lentement** que sur un seul cœur de CPU :

| programme | 1 copie sur 1 cœur CPU | 1 copie sur le GPU |
|---|---|---|
| `freeverb` | 14 µs | 0,56 à 1,5 ms |
| `zita_rev1` | 30 µs | 1,0 à 2,6 ms |

(temps de calcul d'un bloc de 256 échantillons, soit 5,3 ms de son à 48 kHz.)

> **Le point à retenir.** Le GPU n'est pas un processeur plus rapide pour un traitement du son. C'est
> un très grand nombre de processeurs lents. Il n'est rentable que lorsqu'il a
> des milliers de tâches identiques et indépendantes à faire en même temps.

## 3. Pourquoi un programme Faust seul ne va pas plus vite

La première idée consiste à découper un programme en morceaux et à confier
chaque morceau à un groupe d'unités du GPU. La proposition C++ l'a tenté (son
option `-gpu`) : elle tournait **270 à 2 600 fois plus lentement** que le CPU.

La raison tient à la nature de l'audio. La plupart des traitements du son ont
une **mémoire** : la sortie d'un filtre dépend de ses sorties précédentes, un
écho des échantillons de la dernière seconde.

*Une récursion : chaque échantillon a besoin du précédent*

```faust
_ : + ~ *(0.5)
```

On ne peut pas calculer l'échantillon `n` avant l'échantillon `n − 1`. Le
temps impose un ordre, et cet ordre ne se partage pas entre mille cuisiniers.
À l'intérieur d'un programme, il n'y a que quelques morceaux indépendants, et
chacun coûte un lancement sur le GPU.

## 4. L'idée qui marche : beaucoup de copies du même programme

L'ouverture vient d'ailleurs : bien des musiques ont besoin de **milliers de
copies du même petit programme**, chacune avec ses propres réglages, et ces
copies ne dépendent pas les unes des autres.

- **La synthèse additive** : un son construit à partir de milliers de
  partiels, chacun un petit oscillateur avec sa fréquence et son amplitude.
- **La synthèse modale** : une cloche, une plaque ou une salle décrite par
  ses modes de résonance, chacun un petit résonateur.
- **La polyphonie massive** : un nuage, un essaim, une foule de voix.
- **La synthèse granulaire** : des milliers de grains à la fois.
- **Le travail hors ligne** : rendre des milliers de variantes d'un son, ou
  entraîner un modèle différentiable sur de nombreux exemples à la fois
  (§6.4 en mesure un cas).

Là, la cantine est dans son élément : chaque unité du GPU exécute **une
copie** (une *instance*) du programme Faust, toutes les copies avancent
ensemble, échantillon après échantillon, et l'ordre du temps est respecté à
l'intérieur de chaque copie.

> **Définition : Instance.** Une copie en cours d'exécution d'un programme Faust, avec sa propre mémoire
> (les états de ses filtres, ses lignes à retard) et ses propres contrôles. Le
> programme Faust décrit **une** instance ; le nombre d'instances lancées est
> choisi par le programme hôte à l'exécution, il n'est pas écrit dans le code
> Faust.

Conséquence agréable : **aucun nouveau code DSP**. Un programme Faust écrit
pour une voix tourne N fois tel quel. La fonction `compute` générée pour une
instance devient, sans changement, le corps de chaque thread du GPU.

## 5. Ce que fait le compilateur

Si le code Faust ne change pas, que fait le compilateur ? Il change
**l'endroit où vit la mémoire de chaque instance**.

Par défaut, chaque instance garde sa mémoire dans sa propre structure, comme
un classeur par musicien. Sur un GPU, c'est une mauvaise idée : à chaque
instant, les mille unités lisent la même variable dans mille classeurs
différents, éparpillés dans la mémoire, et la mémoire ne suit pas.

`faust-rs` sait ranger la mémoire dans l'autre sens, un tiroir par variable,
avec les valeurs de toutes les instances côte à côte (une « structure de
tableaux »). Quand les mille unités lisent « leur » variable, elles lisent
des valeurs voisines, d'un seul coup.

```text
Une structure par instance              Structure de tableaux (pour le GPU)

voix 0 : état a, état b, état c         état a : voix 0, voix 1, voix 2, ...
voix 1 : état a, état b, état c         état b : voix 0, voix 1, voix 2, ...
voix 2 : état a, état b, état c         état c : voix 0, voix 1, voix 2, ...
```

Pour les programmes qui ont beaucoup de mémoire, cela change tout. À
4 096 voix :

| programme | mémoire par voix | gain dû au rangement |
|---|---|---|
| `freeverb` | 150 Ko | ×31 |
| `karplus32` (32 cordes) | 482 Ko | ×26 |
| `zita_rev1` | 480 Ko | ×20 |

Sans ce rangement, ces programmes ne battent jamais le CPU ; avec lui, ils le
battent. Le rangement ne change aucun calcul : sur le CPU, chaque voix donne
bit pour bit le même résultat qu'une copie ordinaire du programme, et un test
de l'intégration continue le vérifie.

**Garder l'état sous la main.** Une fois la mémoire bien rangée, le code
généré allait encore la chercher à chaque échantillon : relire l'état d'un
filtre, calculer, le réécrire, 48 000 fois par seconde et par copie. C'est
un cuisinier qui retournerait au garde-manger chercher le sel à chaque
geste. Un CPU garde ces valeurs dans ses registres sans qu'on le lui
demande. Mojo, sur le GPU, ne le fait pas, faute de pouvoir prouver que la
mémoire de l'état et les buffers de sortie ne se recouvrent pas.

Depuis le 8 novembre 2026, le compilateur le fait lui-même : `compute` lit
l'état au début du bloc, le garde dans des variables locales pendant toute la
boucle, et ne le réécrit qu'à la fin. Les lignes à retard lues à une position
variable restent en mémoire, puisqu'un buffer circulaire ne tient pas dans
des registres. Là encore aucun calcul ne change : chaque voix reste bit pour
bit identique à une copie ordinaire, et un vérificateur indépendant contrôle
chaque réécriture. Le gain est d'un facteur 2 à 5 :

| programme | état relu à chaque échantillon | état gardé en local |
|---|---|---|
| carillon, 256 cloches, un bloc | 0,84 ms | 0,46 ms |
| apprentissage modal (§6.4), 1 024 modes, une époque | 75 ms | 14,8 ms |
| apprentissage modal, 16 384 modes, une époque | 659 ms | 292 ms |

## 6. Ce qui a été mesuré

Tous les chiffres viennent d'une seule machine, un ordinateur portable
Apple M5 (4 cœurs rapides, 6 cœurs économes, son GPU intégré), à 48 kHz, par
blocs de 256 échantillons. Le budget d'un bloc est de **5,33 ms** : au-delà,
le son se coupe.

Les §6.1 à 6.3 ont été mesurés avant que le compilateur garde l'état en local
(§5). Leurs points de bascule sont donc pessimistes : le GPU gagne
aujourd'hui plus tôt, de combien n'a pas encore été remesuré. Le §6.4 a été
mesuré après.

### 6.1 Le point de bascule

En dessous d'un certain nombre de copies le CPU gagne, au-dessus le GPU
gagne. Face au même programme exécuté une copie à la fois sur les threads du
CPU :

| programme | GPU plus rapide à partir de | gain à 4 096 copies |
|---|---|---|
| passe-tout biquad (`APF`) | 1 024 | ×3,6 |
| oscillateur à table (`osc`) | 1 024 | ×4,8 |
| `karplus32` (32 cordes) | 1 024 | ×4,8 |
| `freeverb`, `zita_rev1` | 4 096 | ×2,2 |

L'ordre de grandeur à retenir : **un millier de copies pour les petits
programmes, quelques milliers pour les gros**. La polyphonie ordinaire (16 à
128 voix) reste loin en dessous.

### 6.2 Un carillon : une leçon d'honnêteté

Un vrai modèle modal : la cloche d'église de `pm.standardBell`
(physmodels.lib), 50 modes par cloche, chaque mode une instance GPU avec sa
fréquence, sa décroissance et son gain. Une cloche sur le GPU reproduit
l'original à 1,5e-4 près.

La première mesure comparait le GPU à son propre code exécuté sur le CPU, et
montrait ×5,7 à ×9,7. Mais ce code, rangé pour le GPU, est 4 à 5 fois plus
lent sur un CPU que le programme qu'écrirait un utilisateur de CPU (une
instance ordinaire par cloche, ses 50 modes dans une seule boucle). Face à ce
programme :

| cloches | modes | GPU | CPU, 4 threads |
|---|---|---|---|
| 37 | 1 850 | 0,64 ms | 0,15 ms |
| 256 | 12 800 | 0,84 ms | 0,97 ms |
| 1 024 | 51 200 | 2,0 ms | 3,9 ms |

Le GPU ne gagne qu'à partir d'environ 220 cloches, d'un facteur 1,9 à 1 024.
Une mélodie sur 37 cloches coûte 9 % d'un cœur de CPU. **La comparaison n'est
juste que face à la meilleure version CPU**, et trouver cette version a divisé
le gain par cinq.

Ce tableau précède l'état gardé en local (§5). Remesurées une fois depuis,
256 cloches prennent 0,46 ms sur le GPU, contre 1,14 ms pour les threads du
CPU mesurés dans le même processus : ×2,5 au lieu de ×1,15. Le carillon
gagne donc bien avant 220 cloches, mais le nouveau point de bascule n'a pas
été mesuré.

### 6.3 Un chœur que seul le GPU peut jouer

Pour trouver un programme qui ait vraiment besoin du GPU, il faut que chaque
copie fasse un vrai travail. C'est le cas du chœur : chaque **partiel**
(`tests/corpus/gpu_choir_partial.dsp`) est une sinusoïde avec son propre
vibrato, une respiration d'amplitude, et une bande de bruit filtrée autour du
partiel, dont le centre suit le vibrato échantillon par échantillon. C'est ce
qui donne son souffle à une voix, et ce qu'une méthode moins coûteuse (la
resynthèse par FFT inverse) ne sait pas exprimer.

La scène : 448 chanteurs en quatre pupitres, chaque harmonique jusqu'à
20 kHz une copie du partiel, les formants de la voyelle « a », une suite de
quatre accords. Cela fait **48 048 partiels**, donc 48 048 petits processeurs
de signal qui tournent en même temps.

- Le meilleur code CPU que Faust produit pour ce partiel (LLVM) coûte 7,4 ns
  par partiel et par échantillon sur un cœur. Avec **les dix cœurs** occupés,
  la machine calcule au plus environ **22 600 partiels** en temps réel, sans
  même les mixer.
- Le GPU calcule les 48 048 partiels, et les mixe, en **3,6 ms par bloc,
  68 % du budget**. Le chœur demande **2,1 fois le CPU entier**.
- Il joue en temps réel pendant 20 s sans coupure, de même que 576 chanteurs
  (61 776 partiels, 84 % du budget).

Sur cette machine, le GPU est le seul moyen de jouer ce chœur. En débit, pour
cette voix, le GPU vaut environ **trente cœurs de CPU**.

Le chœur existe en deux versions. La version **sèche** est celle mesurée
ci-dessus : quatre accords en position serrée, 5 s chacun. La version **dans
une salle** ajoute une réverbération qui tourne sur le CPU (section suivante)
et une partition plus riche :

- les quatre registres sont plus étalés, du do 2 des basses au sol 5 des
  sopranos, et chaque pupitre a son propre vibrato (plus lent et plus étroit
  dans le grave, plus rapide et plus large dans l'aigu), son propre souffle
  et sa place sur scène (sopranos à gauche, puis altos, ténors, basses à
  droite) ;
- après les quatre premiers accords (ré mineur, si bémol, fa, do) viennent
  quatre autres qui explorent davantage l'harmonie : sol mineur 7, mi bémol
  (la sixte napolitaine de ré mineur), la 7 (la dominante, avec sa sensible
  do dièse) et ré majeur (une tierce picarde), avant que la boucle revienne
  à ré mineur ;
- chaque accord est suivi de 3 s de silence, pour qu'on entende seule la
  queue de la salle.

Ses harmoniques s'arrêtent à 16 kHz au lieu de 20 kHz, pour que les basses
plus graves n'augmentent pas le nombre de partiels : 448 chanteurs font
46 592 partiels, calculés en environ 3,5 ms par bloc.

### 6.4 Retrouver les modes d'un son : apprendre sur le GPU

Les sections précédentes jouent des modes dont on connaît déjà les
réglages. Le problème inverse est plus courant : on a un enregistrement
(une cloche, un gong, une plaque frappée) et l'on cherche les modes qui le
reproduisent, c'est-à-dire la fréquence, la décroissance et le gain de
chacun. Un objet réel en a des centaines ou des milliers. Le travail se fait
hors ligne, sur de longs blocs, sans contrainte de latence : c'est le
terrain où le GPU est le plus à l'aise.

**Chaque mode est une copie qui sait comment corriger son erreur.** La voix
(`tests/corpus/gpu_modal_fad_voice.dsp`) est un résonateur frappé par une
impulsion, avec trois contrôles : `freq`, `t60` et `gain`. Grâce à `fad`, la
différentiation automatique de `faust-rs`, elle sort non seulement son son,
mais aussi la façon dont ce son changerait si l'on touchait à chacun de ses
trois réglages :

```faust
process = fad(gain * resonator(freq, t60), (freq, t60, gain));
```

L'hôte additionne les milliers de modes et compare le résultat à la cible.
L'écart, échantillon par échantillon, s'appelle l'*erreur*. Chaque mode
multiplie cette erreur par ses trois dérivées et somme le produit sur le
signal : il sait alors dans quel sens bouger ses réglages. Une telle passe
sur tout le signal s'appelle une *époque*, et l'on répète les époques
jusqu'à ce que l'erreur ne baisse plus.

```text
cible ──────────────────────┐
                            ▼
mode 1 ─┐                ┌───────┐
mode 2 ─┼── somme ──────▶│ écart │── erreur e ──┐
  ...   │                └───────┘              │
mode N ─┘                                       ▼
   ▲          chaque mode : somme de e × ses 3 dérivées
   └────────── nouveaux réglages (un thread GPU par mode)
```

Deux choix rendent cela rapide sur le GPU :

- **Une seule passe par époque.** L'erreur à un instant ne dépend que du
  mix à cet instant. On peut donc avancer bloc par bloc : tous les modes
  calculent un bloc, le mix donne l'erreur de ce bloc, et chaque mode y
  confronte ses dérivées avant de passer au bloc suivant. Aucune dérivée
  n'est gardée au-delà d'un bloc.
- **Chaque mode résout son propre petit problème.** La méthode classique
  (Adam) fait de petits pas dans la bonne direction. Ici, chaque mode calcule
  aussi comment ses trois dérivées se ressemblent, et résout un système de
  trois équations pour sauter directement vers son meilleur réglage
  (Gauss-Newton). Il converge en une quinzaine d'époques là où Adam en
  demande environ 150. Quand deux modes voisins corrigent la même erreur en
  même temps, ils peuvent tous deux dépasser leur cible ; un garde-fou annule
  alors le pas et le raccourcit.

**Le coût d'une époque**, pour une seconde de son. Côté CPU, une copie par
mode est limitée par l'enchaînement de sa récursion. Quatre modes dans un
même programme Faust (`tests/corpus/gpu_modal_fad_group.dsp`), compilés par
LLVM, vont deux fois plus vite : c'est la base honnête.

| modes | GPU | CPU, 10 threads | gain du GPU |
|---|---|---|---|
| 1 024 | 14,8 ms | 42 à 49 ms | ×2,8 |
| 4 096 | 68 ms | 162 à 172 ms | ×2,4 |
| 16 384 | 292 ms | 622 à 680 ms | ×2,1 |

Avant que le compilateur garde l'état en local (§5), le même apprentissage
prenait 75 ms à 1 024 modes sur le GPU et perdait contre le CPU.

**Ce que l'apprentissage retrouve.** Pour vérifier le résultat mode par
mode, la cible est d'abord synthétique : un banc de modes caché, rendu par
la même voix. On part d'un point écarté de la vérité de σ largeurs de bande.
La largeur de bande d'un mode est la finesse de sa raie, d'autant plus étroite
que le mode sonne longtemps. Un mode est compté comme retrouvé si sa
fréquence est juste à 1 % de sa largeur de bande près, et sa décroissance et
son gain à 1 % près.

| départ | modes retrouvés sur 1 024, après 300 époques (Gauss-Newton) |
|---|---|
| σ = 0,25 | 1 021 |
| σ = 0,5 | 1 001 |
| σ = 1 | 914 |
| σ = 2 | 268 |

À σ = 0,5, l'erreur atteint presque son minimum dès la quinzième époque,
soit 0,2 s de GPU.

Deux limites, plus importantes que la vitesse :

- **L'apprentissage affine, il ne cherche pas.** Au-delà d'environ une
  largeur de bande d'écart, un mode ne « voit » plus la raie qu'il devrait
  rejoindre. Un vrai outil commencerait par une analyse du son (repérer les
  pics du spectre) qui place chaque mode assez près, puis laisserait
  l'apprentissage finir le travail.
- **Trop de modes pour le signal, et la réponse n'est plus unique.** À
  4 096 modes entre 100 Hz et 16 kHz, les modes sont plus proches les uns
  des autres que leur largeur de bande. Sur une seconde de son, plusieurs
  jeux de modes expliquent alors le même signal : l'erreur baisse, mais les
  réglages ne rejoignent pas ceux du banc caché. Un signal plus long, ou une
  contrainte sur le nombre de modes, serait nécessaire.

**Sur une vraie cloche.** Pour un enregistrement, chaque mode reçoit en plus
sa propre phase (une seconde amplitude, celle du cosinus,
`tests/corpus/gpu_modal_fad_voice_phase.dsp`). Le point de départ vient du son
lui-même : les pics du spectre, la décroissance de chacun, et les amplitudes
de tous les modes calculées ensemble (`tools/modal_init.py`). On a essayé
deux cloches de Wikimedia Commons :

- **une petite cloche tenue à la main** n'obéit pas au modèle : ses partiels
  glissent de plusieurs hertz et leur volume remonte après coup.
  L'apprentissage n'y gagne presque rien ;
- **la sonnerie des heures de St. Georgen** (St-Gall), une cloche fixe
  frappée par son marteau, s'y prête. Sur 3,8 s du dernier coup, 300 modes
  passent de 6,3 dB (le rapport entre la cible et ce qui reste à expliquer, au
  départ) à 17,9 dB après 1 500 époques, soit 98 % de l'énergie expliquée.
  Ce qui reste : la fin de la décroissance, un doublet du bourdon qui bat, et
  les aigus au-dessus de 2 kHz, où le départ n'avait placé aucun mode.

Plus de modes n'aide pas : à 600, le résultat se dégrade. À 300 modes, le
GPU est sous son point de bascule ; c'est la méthode qui sert ici, pas la
vitesse. La commande est au §9.2.

## 7. CPU et GPU dans le même programme

Rien n'oblige un programme à tourner entièrement sur le GPU, ou entièrement
sur le CPU. Mojo compile **un seul** programme dont les fonctions s'exécutent
sur l'un ou sur l'autre : certaines sont lancées sur le GPU (les *noyaux*),
les autres sont du code CPU ordinaire. Du code Faust peut donc être placé des
deux côtés d'un même programme, chaque morceau là où il est le mieux servi.

Le chœur dans une salle le montre. Sa réverbération (`re.zita_rev1_stereo`,
dans `tests/corpus/gpu_choir_reverb.dsp`) est **un seul** programme appliqué
au mix stéréo : exactement ce que le GPU fait mal (§2). Le travail se partage
donc, bloc après bloc :

```text
   CPU                         GPU                               CPU
+-----------------+    +---------------------------+    +---------------------+
| partition :     |    | 48 048 partiels           |    | réverbération       |
| changements     |--->| (Faust, forme batch)      |    | (Faust, forme       |
| d'accord,       |    |            |              |    |  ordinaire)         |
| contrôles des   |    |            v              |    |         |           |
| partiels        |    | mix : chaque chanteur,    |--->|         v           |
+-----------------+    | puis stéréo               |    | carte son           |
                       +---------------------------+    +---------------------+
                                     2 x 256 échantillons
```

- Le **CPU** déroule la partition : à un changement d'accord, il calcule la
  nouvelle fréquence et le nouveau gain de chaque partiel et les envoie au
  GPU.
- Le **GPU** calcule les 48 048 partiels et les somme en deux canaux :
  3,6 ms par bloc.
- Seuls ces deux canaux, de 256 échantillons chacun, reviennent. Le **CPU**
  les fait passer dans la réverbération et les confie à la carte son :
  environ **50 µs par bloc**, 1 % du budget. La même réverbération sur le GPU
  prendrait 1 à 2,6 ms.

Les deux programmes Faust sont compilés dans le même programme Mojo, sous deux
formes : le partiel sous la forme batch (une structure pour toutes les
copies, sa mémoire rangée pour le GPU), la réverbération sous la forme
ordinaire d'une instance. La version CPU du partiel est elle aussi dans le
programme : c'est la référence contre laquelle on vérifie le GPU.

La règle qui en découle : **mettre sur le GPU ce qui est nombreux, garder sur
le CPU ce qui est unique**. Une réverbération générale, un égaliseur ou un
limiteur sur le mix, un séquenceur, l'entrée MIDI, l'interface : tout cela
reste sur le CPU, à côté de milliers de voix calculées sur le GPU.

Les deux côtés travaillent pour l'instant l'un après l'autre dans chaque bloc
(le CPU attend le mix du GPU, puis applique la réverbération). Ils pourraient
se chevaucher, le CPU traitant un bloc pendant que le GPU calcule le suivant ;
ce n'était pas nécessaire ici.

## 8. Latence : le plus petit buffer audio

Un programme audio ne calcule pas échantillon par échantillon pour la carte
son : il calcule des **blocs** (des buffers) de N échantillons, et la carte
joue un bloc pendant que le suivant se calcule. Un bloc de N échantillons dure
N / 48 000 s : c'est son **budget**. Si un bloc met plus longtemps que son
budget à se calculer, la carte manque d'échantillons et le son **craque**
(un *underrun*, une coupure). Des petits blocs donnent une petite latence, le
délai entre un geste et le son, qui compte pour un instrument joué en direct.

Sur un CPU, le coût d'un bloc est presque entièrement le calcul de ses
échantillons : appeler la fonction `compute` du programme coûte quelques
microsecondes. Les logiciels audio tournent couramment avec des blocs de 32 à
64 échantillons (0,7 à 1,3 ms), et la latence est fixée par le pilote de la
carte son, quelques millisecondes.

Sur un GPU, chaque bloc paie un **coût fixe** avant le premier échantillon :
le CPU prépare le travail, le lance sur le GPU, attend que le GPU ait fini et
recopie le résultat. Ce coût ne diminue pas avec le bloc, et plus le bloc est
petit, plus sa part du budget est grande.

### 8.1 Ce qui a été mesuré

Le coût fixe seul, mesuré avec un chœur de 4 chanteurs (429 partiels, presque
aucun travail), blocs enchaînés sans pause : **0,34 ms** par bloc à 32 ou 64
échantillons, 0,41 ms à 128, 0,55 ms à 256.

En temps réel, c'est pire. Entre deux blocs, le GPU attend la carte son, se
met au repos et baisse son horloge ; chaque bloc paie alors de nouveau le
réveil et la soumission. À 64 échantillons, un bloc coûte environ **1 ms**
quelle que soit la taille du chœur, trois fois le chiffre sans pause, et
certains blocs prennent 3 à 5 ms (la *gigue*). Le chœur en temps réel
(PortAudio, 8 s, en muet) :

| bloc | budget | résultat | bloc médian | pire bloc | latence accordée |
|---|---|---|---|---|---|
| 256 échantillons | 5,33 ms | 448 chanteurs (48 048 partiels) : aucune coupure | 3,6 ms | 10 à 17 ms | 43 ms |
| 128 échantillons | 2,67 ms | 448 chanteurs : aucune coupure | 1,98 ms | 4,9 ms | 30 ms |
| 64 échantillons | 1,33 ms | coupures, même avec 32 chanteurs (3 432 partiels) | 1,0 ms | 3,0 à 3,4 ms | 23 ms |
| 32 échantillons | 0,67 ms | coupures, même avec 8 chanteurs (858 partiels) | 0,57 ms | 2,2 à 3,2 ms | 20 ms |

**Sur cette machine, le plus petit bloc qui joue sans craquer est de 128
échantillons (2,7 ms), avec une latence de sortie d'environ 30 ms.** À 64
échantillons, le bloc médian tient dans son budget, mais le coût fixe ne
laisse aucune marge pour la gigue, et les blocs hors budget s'accumulent plus
vite que le buffer ne peut les absorber. Un chœur plus petit n'y change rien :
en dessous de quelques milliers de partiels, le bloc coûte le coût fixe, pas
le travail.

C'est la principale contrainte pratique du GPU par rapport au CPU. Un CPU
joue un programme léger à 32 échantillons avec quelques millisecondes de
latence ; un GPU, même avec presque rien à calculer, a besoin de 128
échantillons et, avec le lecteur utilisé ici, d'environ 30 ms.

### 8.2 Comment le lecteur pilote PortAudio

Le harnais parle à la carte son par l'API **bloquante** de PortAudio
(`Pa_WriteStream`), depuis le même thread que celui qui pilote le GPU. À
chaque exécution :

1. `Pa_OpenStream` ouvre la sortie par défaut : 2 canaux, flottants 32 bits,
   48 kHz, N échantillons par buffer, une latence *suggérée* de deux blocs,
   et pas de callback (ce qui choisit le mode bloquant). `Pa_GetStreamInfo`
   renvoie la latence réellement accordée.
2. Avant le démarrage du flux, 20 blocs sont calculés et jetés, pour que le
   GPU atteigne son horloge et que les premiers blocs ne soient pas lents.
3. Puis, pour chaque bloc :
   - le CPU applique la partition (à un changement d'accord, il envoie les
     nouveaux contrôles de chaque partiel) ;
   - le CPU lance le travail du GPU (les partiels, puis les deux étages du
     mix) et une copie des 2 x N échantillons mixés en retour, puis **attend**
     le GPU ;
   - avec la salle, le CPU applique la réverbération à ces échantillons ;
   - `Pa_WriteStream` confie les N échantillons à PortAudio. PortAudio les
     garde dans son buffer ; si le buffer est plein, l'appel **attend** que
     la carte en ait joué assez. C'est cette attente qui cale la boucle sur
     la vitesse de la carte son.
   - Si la carte a manqué d'échantillons depuis l'écriture précédente,
     `Pa_WriteStream` renvoie `paOutputUnderflowed` : le harnais compte une
     coupure.

```text
à chaque bloc de N échantillons :

  CPU (thread du harnais)     GPU                buffer PortAudio    carte son
      |                        |                       |                 |
      |-- contrôles (à un ---->|                       |                 |
      |   changement d'accord),|                       |                 |
      |   lancement des        |                       |                 |
      |   partiels et du mix   |                       |                 |
      |<-- 2 x N échantillons -|                       |                 |
      |    mixés (le CPU les   |                       |                 |
      |    attend)             |                       |                 |
      |                        |                       |                 |
      |-- réverbération (version dans la salle)        |                 |
      |                                                |                 |
      |-- Pa_WriteStream (attend tant que le --------->|                 |
      |   buffer est plein)                            |-- joue -------->|
      |                                                |   en continu    |
```

La latence accordée (43 ms à 256 échantillons, 30 ms à 128) est ce buffer :
c'est lui qui permet à un bloc lent (un changement d'accord prend 10 à 17 ms,
trois blocs de 256 échantillons) de passer sans coupure, pourvu que les blocs
suivants soient assez rapides pour le remplir de nouveau. Avec l'API
bloquante, PortAudio ajoute sa propre mise en tampon à celle du pilote, si
bien que la latence dépasse les deux blocs demandés.

### 8.3 Ce qui pourrait la réduire

- **Un lecteur piloté par le callback audio** (l'API à callback de
  PortAudio, ou Core Audio directement) ramènerait la mise en tampon vers le
  minimum du pilote. Mais le callback tourne sur un thread temps réel, qui ne
  doit jamais attendre le GPU : l'attente à la fin de chaque bloc est un appel
  système bloquant, dont la durée n'est pas garantie. La bonne architecture
  est un thread de calcul séparé qui garde quelques blocs d'avance, et une
  file sans verrou entre lui et le callback. La latence est alors la
  profondeur de cette file, que la gigue impose : avec 3 à 5 ms de gigue à 128
  échantillons, deux ou trois blocs, soit environ 6 à 8 ms de file plus le
  buffer propre du pilote. Ni écrit ni mesuré.
- **Recouvrir les blocs** : lancer le bloc k + 1 sur le GPU pendant que le
  bloc k passe par la réverbération et part vers PortAudio masque une partie
  du coût fixe.
- **Garder le GPU éveillé** entre les blocs supprimerait la pénalité du temps
  réel (1 ms au lieu de 0,34 ms à 64 échantillons), au prix de la
  consommation.

Rien de cela ne supprime le coût fixe d'un aller-retour vers le GPU : le GPU
restera derrière le CPU pour les plus petits buffers. Son domaine est le son
qui peut se permettre quelques dizaines de millisecondes : installations,
masses sonores, scènes spatiales, rendu hors ligne, entraînement ; pas le
retour direct d'un instrument joué.

## 9. Essayer

La voie GPU est une piste de test expérimentale, pas encore un produit. Elle
demande une chaîne Mojo (Mojo 1.3 et le paquet `modular`) et, pour écouter,
PortAudio. Les commandes sont dans
[`tests/impulse-tests/README.md`](../tests/impulse-tests/README.md).

### 9.1 Installer la chaîne d'outils

Les mesures ont utilisé une version nocturne de Mojo 1.3
(`1.3.0.dev2026110705`, avec `modular` `26.8.0.dev2026110705`) et PortAudio
de conda-forge, installés avec [pixi](https://pixi.sh) dans un environnement
dédié. Un fichier `pixi.toml` dans un répertoire vide :

```toml
[workspace]
channels = ["https://conda.modular.com/max-nightly", "conda-forge"]
name = "mojo-gpu"
platforms = ["osx-arm64"]

[dependencies]
mojo = "*"
modular = "*"
portaudio = ">=19.7.0,<20"
```

Puis `pixi install` dans ce répertoire. Les deux valeurs dont les commandes
ci-dessous ont besoin sont la commande Mojo et le répertoire de la
bibliothèque PortAudio :

```bash
MOJO="pixi run --manifest-path <env>/pixi.toml -q mojo"
```

```bash
PORTAUDIO_LIB=<env>/.pixi/envs/default/lib
```

Les versions nocturnes de Mojo changent souvent ; fixer `modular` à la
version ci-dessus reproduit la chaîne mesurée. La première construction d'une
piste prend environ une minute (la compilation Mojo du harnais et des
programmes générés).

### 9.2 Jouer et rendre en fichier

Le chœur, sec :

```bash
make -C tests/impulse-tests mojo-gpu-choir MOJO=<commande mojo> PORTAUDIO_LIB=<rép> POLY_AUDIBLE=audible
```

Le chœur dans la salle, la réverbération sur le CPU :

```bash
make -C tests/impulse-tests mojo-gpu-choir-reverb MOJO=<commande mojo> PORTAUDIO_LIB=<rép> POLY_AUDIBLE=audible
```

Une fois construit, le programme se joue directement, et la salle se règle :
`dry` et `wet` sont les niveaux du son direct et du son réverbéré (0,4 et 2
par défaut, la salle environ 3 dB au-dessus du chœur), `t60` le temps de
décroissance en secondes (5 par défaut), `gap` le silence après chaque accord
(3 s par défaut) :

```bash
tests/impulse-tests/build/mojo-gpu-choir/choir play 448 30 audible
```

```bash
tests/impulse-tests/build/mojo-gpu-choir/choir play 448 64 audible reverb wet=3 t60=7 gap=4
```

Un cycle complet de la version dans la salle (huit accords et leurs
silences) dure 64 s. Mesurez ou écoutez un seul chœur à la fois : deux chœurs
lancés ensemble se partagent le GPU, chacun met environ deux fois plus de
temps par bloc, et les deux se coupent.

Les deux versions peuvent aussi être rendues hors temps réel en fichiers WAV,
pour les écouter sans la chaîne d'outils : `make -C tests/impulse-tests
mojo-gpu-choir-render` écrit `build/mojo-gpu-choir/choir-dry.wav` (20 s) et
`choir-reverb.wav` (69 s). Le GPU les calcule plus vite que le temps réel.

Le carillon :

```bash
make -C tests/impulse-tests mojo-gpu-bells MOJO=<commande mojo> PORTAUDIO_LIB=<rép> BELLS=37 BELLS_SCORE=chime POLY_AUDIBLE=audible
```

Chaque piste vérifie le GPU contre le CPU, chronomètre les deux, puis joue.

L'apprentissage des modes (§6.4), sans PortAudio :

```bash
make -C tests/impulse-tests mojo-gpu-modal-fad MOJO=<commande mojo> MODAL=4096
```

Il vérifie les dérivées et le code GPU, chronomètre une époque sur le GPU
(avec l'état gardé en local, puis sans) et sur le CPU, puis lance un
apprentissage de `MODAL_EPOCHS` époques depuis un départ à `MODAL_SIGMA`
largeurs de bande, avec `MODAL_UPDATE` (`gn` ou `adam`).

Le même apprentissage sur un enregistrement que l'on fournit (il faut aussi
sox et numpy) :

```bash
make -C tests/impulse-tests mojo-gpu-modal-fad-bell MOJO=<commande mojo> BELL_WAV=<fichier> BELL_START=<s> BELL_SECONDS=4
```

Il écrit les modes trouvés, et la cible, le modèle et le résidu en WAV au
même gain, dans `tests/impulse-tests/build/mojo-gpu-modal-fad-bell/`.

Côté Faust, c'est du Faust ordinaire. Le partiel du chœur commence ainsi :

```faust
freq = hslider("freq", 440, 1, 20000, 0.001);
gain = hslider("gain", 0.01, 0, 1, 0.0001);
vrate = hslider("vrate", 5.5, 0, 20, 0.001);
...
process = gaing * swell * ((1 - noisiness) * partial + noisiness * nscale * bandpass(noise));
```

Rien n'y dit « 48 048 » : c'est le programme hôte qui crée autant de copies
et règle les contrôles de chacune.

### 9.3 Écrire son propre programme GPU

Pour un développeur qui veut faire tourner un autre programme Faust sur le
GPU, voici les pièces.

- **La forme batch du programme.** `faust-rs -lang mojo` écrit la forme
  ordinaire, une structure par instance. La forme batch, une structure pour N
  instances, n'a pas encore d'option en ligne de commande ; l'exemple du
  compilateur l'écrit :

  ```bash
  cargo run --release -p compiler --example mojo_batch -- voice.dsp --cn voicedsp -o voice.mojo
  ```

  L'état y est gardé en local pendant la boucle (§5) ; `--no-registers`
  écrit l'ancienne forme, pour comparer.

- **Ce qu'offre la structure batch.** Elle est construite sur un seul buffer
  d'état, alloué par l'hôte sur le GPU : `state_bytes(voices)` donne sa
  taille, et `voicedsp(voices, state)` construit la structure dessus.
  `class_init(rate)` s'exécute une seule fois pour toutes les instances ;
  toutes les autres méthodes prennent l'instance en premier :
  `instance_init(voice, rate)`, `set_control(voice, index, value)`,
  `get_control(voice, index)`, `compute(voice, count, inputs, outputs)`.
  L'indice d'un contrôle est sa position dans `build_user_interface`, que
  l'hôte lit une fois pour associer les noms aux indices.
- **Ce qu'écrit l'hôte.** Les *noyaux* GPU, chacun exécutant une instance
  par thread du GPU : l'un appelle `instance_init`, un autre `set_control`
  avec une valeur par instance, un autre `compute`. Les tables de pointeurs
  d'entrée et de sortie de chaque instance (un mode y lit la frappe de sa
  cloche). Le mix, de préférence en deux étages : un thread GPU par
  échantillon sommant tous les modes rendait le carillon plus lent que le
  CPU ; un par cloche, puis un par échantillon sur les cloches, l'a rendu
  plus rapide. Et la boucle du §8.2 autour de PortAudio.
- **Trois règles apprises sur Metal**, chacune un échec silencieux quand on
  l'enfreint : tout buffer qu'un noyau atteint doit être l'un de ses
  arguments ; les tables de pointeurs doivent vivre dans la mémoire du GPU ;
  les arguments des noyaux doivent être de taille fixe.
- **Des modèles pour commencer**, dans `tests/impulse-tests/archs/` :
  `choirmojogpubatch.mojo` (un programme, des milliers d'instances avec leurs
  propres contrôles, une réverbération sur CPU, PortAudio, rendu en fichier),
  `bellsmojogpubatch.mojo` (deux programmes enchaînés sur le GPU, la frappe
  nourrissant ses modes), `polymojogpubatch.mojo` (une polyphonie pilotée
  par des notes écrites d'avance) et `modalfadmojogpubatch.mojo` (des
  milliers d'instances avec leurs dérivées, les sommes et la mise à jour de
  chaque instance sur le GPU).

## 10. Les limites actuelles

L'approche est réelle, mesurée et validée, mais étroite. Ses limites comptent
autant que ses gains.

**Elle ne convient qu'à une forme de problème.** Des milliers de copies
indépendantes du même programme. Un programme lourd et unique (une grande
réverbération, un gros modèle physique) tourne plus lentement sur le GPU, et
la polyphonie ordinaire est sous le point de bascule. Un grand maillage (une
membrane, une plaque simulée point par point), une convolution par FFT ou une
matrice de mixage sont parallèles *à l'intérieur* d'un programme : Faust les
écrit comme un seul programme, et cette approche ne les aide pas.

**Latence et taille de bloc** (§8). Chaque bloc paie un coût fixe (0,34 ms
sans pause, environ 1 ms en temps réel à 64 échantillons) avant le moindre
calcul de son, plus plusieurs millisecondes de gigue. Le plus petit bloc qui a
joué sans craquer est de 128 échantillons, avec environ 30 ms de latence de
sortie par l'API bloquante de PortAudio ; un CPU joue les programmes légers à
32 ou 64 échantillons avec quelques millisecondes. Cela convient à une
installation, à une pièce de masse sonore ou au rendu hors ligne, pas au
retour direct d'un instrument joué en direct.

**Précision.** Les GPU grand public calculent en virgule flottante 32 bits
(`f32`), pas en 64 bits. Les filtres à fort facteur de qualité, les très
basses fréquences, les réseaux à rétroaction sensibles aux arrondis
s'écartent du résultat CPU. La construction du chœur est tombée sur un piège
classique : un glissement de fréquence calculé en `f32` s'arrête avant sa
cible (d'environ 0,4 cent), en un point légèrement différent sur GPU et sur
CPU, et les phases dérivent ensuite. La voix ne fait plus glisser que son
gain.

**Tous les programmes Faust ne peuvent pas aller sur le GPU.** Sur les 133
programmes de la suite de tests de réponse impulsionnelle, 131 y tournent.
Les deux autres demandent ce que le GPU Apple ne peut pas donner : `sound`
lit un soundfile, que le backend Mojo refuse (comme celui du C++), et `math`
calcule une puissance entière en 64 bits, que le GPU n'a pas. `-double` y est
impossible pour la même raison, et le backend Mojo refuse les modes
`-ec`/`-os`. Sur les 131, environ 120 donnent le résultat du CPU à 1e-5 près.
Huit s'en écartent par leur rétroaction, de 0,08 % (`thru_zero_flanger`) à
45 % (`phaser_flanger`) de leur crête sur 15 000 échantillons : l'écart
commence au dernier chiffre (les deux compilateurs produisent un code flottant
différent, et le GPU met à zéro les très petits nombres), et la rétroaction
l'amplifie.

**Tout le monde doit faire la même chose.** Les unités d'un groupe exécutent
la même instruction. Si des voix prennent des chemins différents (une cadence
différente par voix, des itérations qui varient), elles s'attendent : le
résultat reste juste, la vitesse baisse.

**Mémoire.** Chaque copie a sa propre mémoire : 4 096 copies de `zita_rev1`
demandent 2 Go.

**Le programme hôte est écrit à la main.** Faust décrit une copie. Nourrir
les copies, les relier (la frappe d'une cloche nourrit ses 50 modes), les
sommer en un mix stéréo et piloter la carte son s'écrivent aujourd'hui dans
des harnais de test Mojo. La sortie « batch » (une structure pour N voix)
n'a pas encore d'option en ligne de commande ; on y accède par l'API Rust du
compilateur (§9.3).

**Mojo n'est pas plus rapide que le C++ sur le CPU.** Sur le partiel du
chœur, le code Mojo généré tournait cinq fois plus lentement sur le CPU que
le même programme compilé par LLVM depuis le C++. L'intérêt de Mojo, ici,
c'est le GPU ; les comparaisons CPU de ce document utilisent le code LLVM.

**Une chaîne d'outils jeune.** Mojo change d'une version à l'autre, et ce
travail a rencontré trois défaillances dans sa voie GPU, dont deux
silencieuses. Les pistes sont hors de l'intégration continue (qui n'a pas
Mojo), et le backend Mojo C++ dont `faust-rs` suit le contrat n'est pas
encore fusionné, si bien que ce contrat peut encore bouger.

**La comparaison dépend du côté CPU.** Les chiffres comparent le GPU au
meilleur code CPU que Faust produit aujourd'hui. Une version CPU vectorisée à
la main entre les voix (quatre ou huit voix par instruction) n'a pas été
mesurée, et elle pourrait réduire l'écart, peut-être le combler pour les
petits programmes. Le carillon a montré combien une base CPU plus juste peut
changer une conclusion.

**Un GPU partagé avec d'autres travaux.** Le GPU n'est pas réservé au son.
Pendant ces mesures, deux chœurs lancés en même temps l'ont partagé : chacun
mettait environ deux fois plus de temps par bloc (5,5 à 6,1 ms au lieu de
3,6) et les deux craquaient. Tout ce qui utilise le GPU (un autre programme
audio, une vidéo, l'affichage d'une application) prend sa part du même
budget, et un programme audio n'a aucun moyen de se réserver le GPU.

**Une seule machine.** Tout a été mesuré sur un Apple M5 avec son GPU
intégré. Non mesuré : la consommation et la chaleur, un GPU chargé par un
affichage lourd (dans une station audionumérique, sous une interface 3D), les
GPU séparés où les données traversent un bus.

## 11. À retenir

| question | le GPU vaut l'essai | rester sur le CPU |
|---|---|---|
| Combien de copies indépendantes ? | des milliers ou plus | une, ou quelques centaines |
| Où est le parallélisme ? | entre les copies | à l'intérieur d'un programme |
| Travail par copie | un vrai travail par échantillon | quelques opérations (les regrouper sur le CPU) |
| Latence | blocs de 128 échantillons ou plus, 30 à 45 ms acceptables, ou hors ligne | blocs de 32 à 64 échantillons, retour direct en jeu live |
| Précision | `f32` suffit | besoin de `f64`, rétroaction sensible aux arrondis |

- `faust-rs -lang mojo` écrit du code Mojo pour n'importe quel programme
  Faust, avec les mêmes résultats que le C++ sur le CPU.
- Des milliers de copies d'un programme Faust peuvent tourner sur le GPU,
  sans changement ; le compilateur range leur mémoire pour lui.
- Dans ce régime, le GPU apporte de ×2 à ×5 par rapport aux threads du CPU
  pour des programmes d'un vrai poids, et il joue au moins un programme, un
  chœur de 48 000 partiels riches, que le CPU entier ne peut pas jouer.
- Pour l'apprentissage hors ligne, par exemple retrouver des milliers de
  modes par `fad`, le GPU apporte ×2 à ×3 face aux dix cœurs du CPU. Il
  affine un bon point de départ ; il ne remplace pas l'analyse qui le donne.
- Le code CPU et le code GPU se mélangent dans le même programme Mojo : les
  milliers de copies sur le GPU, les programmes uniques (une réverbération
  sur le mix, un séquenceur) sur le CPU.
- Pour tout le reste (un programme unique, la polyphonie ordinaire, les
  petits blocs, le `f64`), le CPU reste la bonne place.

## Voir aussi

- [`vector-scheduling-synthesis-fr.md`](vector-scheduling-synthesis-fr.md) :
  l'autre direction du parallélisme, à l'intérieur du processeur (SIMD), y
  compris des instances qui avancent au même pas.
- [`fad-rad-synthesis-fr.md`](fad-rad-synthesis-fr.md) : la différentiation
  automatique, qui rejoint le GPU dans l'entraînement hors ligne (§6.4).
- [`user-cli-guide-en.md`](user-cli-guide-en.md) : l'option `-lang mojo`.
