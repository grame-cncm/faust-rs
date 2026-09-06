---
title: "Note : les primitives ondemand / upsampling / downsampling dans faust-rs"
author: "Claude Opus 4.8"
date: "2026-07-21"
---

# Domaines d'horloge dans `faust-rs` : `ondemand`, `upsampling`, `downsampling`

Version anglaise : [ondemand-note-en.md](ondemand-note-en.md) (même contenu ;
garder les deux synchronisées en cas de modification).

Cette note présente les trois primitives de domaine d'horloge du point de vue du
programmeur Faust. Elle ne suppose aucune connaissance du fonctionnement interne
du compilateur.

En Faust ordinaire, tous les signaux avancent au même rythme : un échantillon
par tick, pour tout le programme. Les primitives de domaine d'horloge rompent
cette hypothèse. Elles permettent à une sous-expression de s'exécuter **à son
propre rythme** — moins souvent, plus souvent, ou seulement quand quelque chose
se produit — pendant que le reste du programme continue au rythme audio.

```faust
ondemand(C)
upsampling(C)
downsampling(C)
```

## 1. L'horloge est une entrée supplémentaire

Les trois primitives prennent une expression et renvoient une expression
*enveloppée* ayant **une entrée de plus que le corps** :

> si `C : u → v` alors `ondemand(C) : u+1 → v`

Cette **première** entrée supplémentaire est l'horloge `H`. C'est un signal
Faust ordinaire : on peut le calculer, le conditionner, le dériver d'un élément
d'interface.

```faust
// Le corps ne s'exécute que tant que le bouton est maintenu.
process = (button("gate"), _) : ondemand(*(2));
```

La même règle d'arité vaut pour `upsampling` et `downsampling`.

## 2. Ce que chaque primitive fait de l'horloge

| Primitive | Lecture de l'horloge | Effet par tick externe |
|---|---|---|
| `ondemand`, intervalle d'horloge ⊆ [0,1] | exécuter si `H ≠ 0` | le corps s'exécute 0 ou 1 fois |
| `ondemand`, intervalle d'horloge dépassant [0,1] | exécuter `H` fois | le corps s'exécute `H` fois |
| `upsampling` | `H` = facteur | le corps s'exécute `H` fois |
| `downsampling` | `H` = période | le corps s'exécute un tick sur `H` |

Les deux lignes `ondemand` ne sont pas deux primitives, et le choix ne se
déclare pas : le compilateur l'infère de l'**intervalle de valeurs** du signal
d'horloge.

- Si l'intervalle inféré est contenu dans **[0,1]**, l'horloge est lue comme une
  condition et le corps s'exécute au plus une fois, dès que `H ≠ 0`.
- Si l'intervalle est plus large, l'horloge est lue comme un **compte** et le
  corps s'exécute `H` fois.

Attention à ce que le premier cas ne dit *pas* : c'est l'intervalle qui doit
être inclus dans [0,1], pas chaque valeur qui doit être 0 ou 1. Une horloge
pouvant valoir `0.5` a bien un intervalle ⊆ [0,1] : c'est donc une condition —
et comme `0.5 ≠ 0`, le corps s'exécute **une fois**, pas une demi-fois. Il n'y a
pas d'exécution fractionnaire ; pour obtenir un compte, donnez à l'horloge un
intervalle qui dépasse 1.

Les horloges constantes sont simplifiées très tôt :

- `H == 0` — le corps ne s'exécute jamais et les sorties sont remplacées par `0` ;
- une horloge constante non nulle se réduit à la structure fixe correspondante.

Un `ondemand` à horloge littérale ne coûte donc rien à l'exécution : ces
primitives ne sont « dynamiques » que si l'horloge l'est.

## 2 bis. `ma.SR` à l'intérieur d'un domaine

`upsampling` et `downsampling` changent le *rythme* du corps, donc aussi ce que
le corps entend par « la fréquence d'échantillonnage ». `ma.SR` est adapté
automatiquement :

| Contexte | Valeur de `ma.SR` dans le corps |
|---|---|
| `upsampling(C)` d'horloge `H` | `SR * H` |
| `downsampling(C)` d'horloge `H` | `SR / H` |
| US/DS imbriqués | les facteurs se composent |
| `ondemand(C)` | **inchangé** — toujours le `SR` externe |

Vérifié sur le C++ émis : sous `upsampling` d'horloge 2, la constante devient
`fSampleRate * 2` ; sous `downsampling` d'horloge 4, `fSampleRate * 0.25` ; et
`upsampling(2)` enveloppant `downsampling(4)` donne `fSampleRate * 0.5`,
autrement dit toute la pile de facteurs est déroulée.

C'est le comportement souhaité la plupart du temps : un filtre dont les
coefficients sont calculés à partir de `ma.SR` dans un corps `upsampling` est
*automatiquement* accordé au rythme suréchantillonné, sans rien à passer à la
main.

**La ligne `ondemand` est le piège.** `ondemand` n'adapte pas `ma.SR`, et ne le
peut pas : son rythme de déclenchement dépend du signal d'horloge à l'exécution,
il n'existe donc aucun rapport constant à replier dans `SR`. Un corps qui calcule
ses coefficients depuis `ma.SR` dans un `ondemand` sera donc accordé au rythme
*externe*, et non à son propre rythme de déclenchement. Si votre corps a besoin
de son rythme effectif, calculez-le à l'extérieur et passez-le en entrée
ordinaire.

## 3. La subtilité qui compte : le temps est local au domaine

C'est le point qui surprend, et c'est tout l'intérêt de la construction.

À l'intérieur d'un domaine d'horloge, **le temps avance au rythme du domaine,
pas au rythme audio**. Un retard d'un échantillon dans un corps `ondemand`
correspond à un *déclenchement* de retard, pas à un échantillon audio. Cela vaut
pour toute construction à état : lignes de retard, récursion (`~`), tables et
accumulateurs comptent tous en *temps de déclenchement*.

```faust
// `prev` est la valeur précédente produite *pendant que la porte était ouverte*,
// et non la valeur d'il y a un échantillon audio.
process = (button("gate"), _) : ondemand(+ ~ _);
```

Pour un historique au rythme audio, gardez l'état à l'extérieur du domaine et
passez-le en entrée. Pour un historique par événement — un compteur
d'événements, la trame précédente, une valeur maintenue entre deux
déclenchements — placez-le à l'intérieur. Se tromper de côté est la source de
confusion la plus fréquente, et l'erreur est silencieuse : les deux versions
compilent.

« À l'intérieur » se décide sur le texte, pas sur la valeur. Une *définition*
référencée dans le corps (`ba.time`, un oscillateur, une lecture de table, un
signal retardé, une locale de `with`) est instanciée à nouveau dans le domaine
du corps, avec son propre état qui avance en temps de déclenchement, même si la
même définition est aussi utilisée à l'extérieur : le compilateur annote
chaque primitive à état construite dans un domaine avec ce domaine, si bien
que les deux instances ne partagent jamais leur état. Ce qui atteint le corps
depuis l'extérieur comme une *valeur* — une entrée du bloc, un paramètre de
lambda lié à l'extérieur — est le signal extérieur échantillonné au
déclenchement.

```faust
t = ba.time;
process = (clock, t) : ondemand(\(u).(u, float(t)));
// u        : le temps audio, échantillonné à chaque déclenchement
// float(t) : un nouveau compteur de déclenchements
```

## 4. Cas d'utilisation typiques

**Calcul au rythme de contrôle.** Tout ce qui n'a pas besoin d'être recalculé
48000 fois par seconde : suiveurs d'enveloppe alimentant un affichage, logique
de lissage de paramètres, analyse coûteuse. Enveloppez dans `downsampling` et
choisissez une période.

```faust
process = (256, _) : downsampling(analyse_couteuse);
```

**Traitement déclenché par événement.** Un corps qui ne doit s'exécuter que
lorsque quelque chose se produit : note-on, franchissement de seuil, bouton.
`ondemand` à horloge 0/1 fait exactement cela et, contrairement à un
`select2`, ne *calcule pas les deux branches* : le corps ne s'exécute
véritablement pas.

**Suréchantillonnage d'une non-linéarité.** Faire tourner un saturateur ou un
oscillateur à un multiple du rythme audio pour repousser le repliement, avec
`upsampling`. Attention : la primitive contrôle le *rythme d'exécution* — les
filtres anti-repliement autour restent à votre charge.

**Traitement par trames / spectral.** Combiné à la primitive `il.interleave(N,
FX)` de `interleave.lib`, `ondemand` permet à un opérateur de trame de
s'exécuter une fois tous les `N` échantillons, ce qui rend la FFT par trame et
le traitement de type STFT exprimables en Faust pur. `il.interleave(N, id)` vaut
exactement `@(N-1)` : la latence d'aller-retour de la construction est de `N-1`
échantillons. Voir
[ondemand-fft-spectral-comparison-en.md](ondemand-fft-spectral-comparison-en.md).

## 5. Calcul spectral avec `ondemand` et `interleave.lib`

### Du flux audio à l'opérateur de trame

Les opérateurs FFT de `analyzers.lib` sont des circuits spatiaux : une
transformée de taille `N` attend les `N` échantillons d'une trame au même
instant logique. `libraries/interleave.lib` fournit l'enrobage de flux qui relie
un tel opérateur de trame à un flux Faust ordinaire, échantillon par
échantillon :

```text
flux audio
  -> serialize_in(N)
  -> frame_clock(N) + ondemand(FX)
  -> serialize_out(N)
  -> flux audio
```

`serialize_in(N)` expose les `N` derniers échantillons sous forme de `N` voies
parallèles. L'horloge booléenne se déclenche quand la fenêtre est complète ;
`ondemand(FX)` n'exécute donc la FFT, ou l'effet spectral complet, qu'une fois
par trame. `serialize_out` maintient, retarde et somme les voies de résultat
pour reconstruire un flux. L'opérateur pratique `il.interleave(N, FX)` regroupe
ce chemin sans recouvrement ; pour des fenêtres recouvrantes et
l'overlap-add, utiliser `il.interleave_hop(N, hop, FX)`.

Pour une analyse sans resynthèse, les bins peuvent rester des signaux maintenus
au rythme des trames :

```faust
il = library("interleave.lib");
an = library("analyzers.lib");
si = library("signals.lib");

N = 128;
fftFX(NN) =
    par(i, NN, (_, 0))
    : an.c_bit_reverse_shuffle(NN)
    : an.fftb(NN);

process =
    il.serialize_in(N)
    : (il.frame_clock(N), si.bus(N))
    : ondemand(fftFX(N));
```

Pour un effet temps-spectre-temps, `FX` complexifie la trame, la transforme,
modifie les bins complexes, puis effectue la transformée inverse. Cet exemple
compact réalise un passe-bas « brick-wall » sur 16 points :

```faust
il = library("interleave.lib");
an = library("analyzers.lib");

N = 16;
kc = 4;
gain(m) = float(min(m, N-m) <= kc);
scaleBin(g) = *(g), *(g);
mask = par(m, N, scaleBin(gain(m)));

lowpass(NN) =
    par(i, NN, (_, 0))
    : an.fft(NN)
    : mask
    : an.ifft(NN)
    : par(i, NN, (_, !));

process = il.interleave(N, lowpass(N));
```

Compiler les exemples qui importent la bibliothèque locale avec `-I
libraries`, en plus du chemin contenant les bibliothèques Faust standard.

### Exemples DSP vérifiés

Le corpus contient des programmes complets utilisables comme points de départ :

- [ondemand_fft_framed_128.dsp](../tests/corpus/ondemand_fft_framed_128.dsp)
  expose les bins FFT maintenus pour les analyseurs ou les pertes spectrales ;
- [ondemand_fft_roundtrip_id_016.dsp](../tests/corpus/ondemand_fft_roundtrip_id_016.dsp)
  vérifie la reconstruction FFT/IFFT exacte sans recouvrement, après la latence
  de `N-1` ;
- [ondemand_fft_lowpass_016.dsp](../tests/corpus/ondemand_fft_lowpass_016.dsp),
  [ondemand_fft_highpass_016.dsp](../tests/corpus/ondemand_fft_highpass_016.dsp)
  et [ondemand_fft_bandpass_016.dsp](../tests/corpus/ondemand_fft_bandpass_016.dsp)
  appliquent des masques spectraux à symétrie hermitienne ;
- [ondemand_fft_fastconv_032.dsp](../tests/corpus/ondemand_fft_fastconv_032.dsp)
  effectue une convolution par trame en multipliant les bins complexes ;
- [ondemand_stft_robot_ola_016.dsp](../tests/corpus/ondemand_stft_robot_ola_016.dsp)
  combine fenêtre de Hann, recouvrement à 50 %, overlap-add et robotisation par
  conservation de la magnitude ;
- [ondemand_stft_pv_freqshift_016.dsp](../tests/corpus/ondemand_stft_pv_freqshift_016.dsp)
  conserve l'état de phase de chaque bin dans le domaine de trame pour réaliser
  un décalage fréquentiel par vocodeur de phase ;
- [ondemand_stft_denoiser_1024.dsp](../tests/corpus/ondemand_stft_denoiser_1024.dsp)
  est une porte spectrale de type Wiener de plus grande taille.

### Limites de compilation et du code généré

`ondemand` corrige le principal problème de fréquence d'exécution : la
transformée s'exécute une fois par saut, et non à chaque échantillon audio. Il
ne transforme toutefois pas la FFT en primitive opaque du backend. Le
compilateur actuel développe `an.fft(N)`/`an.ifft(N)` en un graphe d'expressions
scalaires de papillons, avec partage par hash-consing, avant la génération de
code.

Cela a deux conséquences pratiques :

- **La compilation croît avec la transformée.** Même avec l'élimination des
  sous-expressions communes dans le bloc cadencé, le compilateur doit
  construire, typer, transformer et émettre un graphe en `O(N log N)`. Une
  grande transformée consomme bien plus de temps et de mémoire qu'un appel à
  une bibliothèque FFT précompilée, et la définition récursive de la FFT Faust
  peut nécessiter une profondeur d'évaluation augmentée. Le débruiteur vérifié
  sur 1024 points utilise `FAUST_RS_DEFAULT_EVAL_MAX_DEPTH=4096` ; c'est
  volontairement un exemple lourd (environ 0,5 million de lignes interpréteur
  et approximativement 40 secondes de compilation sur la machine de
  validation).
- **La transformée générée est scalaire et entièrement spécialisée.** Les
  petites FFT peuvent profiter des twiddles repliés en constantes, de l'absence
  de planification et de boucle d'indexation interne. Pour les grandes tailles,
  le volume de code, la pression sur le cache d'instructions, les débordements
  de registres et le pic de calcul au tick de trame deviennent limitants. Le
  chemin actuel calcule aussi une FFT complexe pour une entrée réelle et ne
  peut égaler une implémentation optimisée utilisant des noyaux FFT réels, des
  boucles étagées adaptées au cache, du SIMD explicite ou de l'assembleur
  spécifique à l'architecture.

Ce sont les limites de l'abaissement actuel, pas de la sémantique
`ondemand`. Une future primitive FFT, ou un IR FFT structuré au niveau des
backends, pourrait conserver l'exécution au rythme des trames tout en produisant
un noyau compact, bouclé ou fourni par une bibliothèque. Pour l'instant, les
petites et moyennes transformées constituent la zone d'utilisation naturelle ;
mesurez le temps de compilation et le pire coût CPU au tick de trame avant
d'utiliser une grande FFT dans une application temps réel.

Enfin, le tramage reste sous la responsabilité du programmeur :
`il.interleave(N, FX)` a une latence aller-retour fixe de `N-1` et utilise des
trames rectangulaires sans recouvrement. Avec `interleave_hop`, choisir des
fenêtres d'analyse et de synthèse satisfaisant la condition d'overlap-add
requise. Un changement de durée reste dépendant d'un découplage de rythme
extérieur au modèle Faust synchrone à une entrée et une sortie.

## 6. Liens avec FAD et RAD

Les domaines d'horloge sont le véhicule pratique de l'**apprentissage dans le
graphe** — la motivation applicative de toute cette machinerie. Voir
[fad-note-en.md](fad-note-en.md) et [rad-usage-en.md](rad-usage-en.md) pour les
primitives de différentiation elles-mêmes.

**Apprentissage au rythme de contrôle.** Un pas de gradient n'a pas besoin de
s'exécuter à chaque échantillon. Envelopper un optimiseur dans un domaine
découple le rythme d'adaptation du rythme audio. Avec
[`optimizers.lib`](../libraries/optimizers.lib) (préfixe `op`) et l'horloge de
trame d'`interleave.lib` :

```faust
// Toute la boucle en temps de tir : un pas d'optimiseur tous les 64
// échantillons, sur les entrées du bloc (excitation et cible échantillonnées au tir).
learn(xi, ti) = op.descend_1D(\(g).(op.mse(g * xi, ti)), op.adam_g(0.02, 0.9, 0.999, 1e-8), -4, 4, 0, 0);
g = (il.frame_clock(64), x, target) : ondemand(learn);
```

**Adaptation déclenchée par événement.** `ondemand` à horloge 0/1 donne
« n'adapter que tant que cette porte est ouverte », ce qui permet de geler un
paramètre appris en dehors d'une phase d'entraînement sans ajouter de branche
dans le chemin audio.

**Gradients décimés.** Calculer une perte au rythme audio mais ne mettre à jour
qu'à un rythme plus lent, en gardant la partie coûteuse de la passe arrière dans
un domaine plus lent. C'est ce que font `op.descend_1D_clocked` …
`op.descend_5D_clocked` : le gradient est calculé à chaque échantillon,
moyenné sur la trame par `op.frame_mean`, et le pas est pris dans un bloc
`ondemand`, si bien que l'état du moteur avance une fois par trame — un pas de
mini-lot :

```faust
// Gradient au rythme audio, un pas SGD par trame de 64 échantillons : exact en
// quelques trames.
g = op.descend_1D_clocked(il.frame_clock(64), loss, op.sgd_g(0.5), -4, 4, 0, 0);
```

**DDSP par trames.** Avec `interleave`, une perte spectrale différentiable
devient exprimable : FFT de la trame, comparaison à un spectre cible,
différentiation du résultat. La section 10 de
[fad-rad-synthesis-fr.md](fad-rad-synthesis-fr.md) et la section 11 de
[optimizers-ddsp-tutorial-fr.md](../libraries/optimizers-ddsp-tutorial-fr.md)
montrent un gain appris sur une perte FFT à 8 points, un pas d'optimiseur par
trame, la trame étant passée au bloc comme entrées nommées.

**Une règle à retenir :** différentiation et domaines d'horloge se composent
*à l'intérieur* d'un domaine, mais une dérivée ne traverse pas une **frontière**
de domaine. `fad` dans un corps `ondemand` est pris en charge, et ses tangentes
sont validées numériquement contre des différences finies. Différentier un
signal qui entre dans un domaine ou en sort est une autre affaire : le
compilateur dispose d'un diagnostic dédié lorsque la différentiation automatique
atteint une frontière de domaine qu'elle ne peut franchir (`FRS-PROP-0004`). Si
vous construisez une boucle d'apprentissage, gardez la graine, la perte et la
mise à jour dans le **même** domaine.

## 7. Remarques pratiques et limites actuelles

- L'horloge est un signal : elle peut elle-même être calculée dans un autre
  domaine. L'imbrication fonctionne, mais raisonnez en temps de déclenchement à
  chaque niveau — les rythmes se multiplient.
- `ondemand` dont l'horloge dépasse 1 exécute le corps `H` fois par tick. Une horloge
  issue d'un calcul non borné peut donc rendre un tick audio arbitrairement
  coûteux ; bornez-la si elle provient d'une entrée utilisateur.
- Les domaines d'horloge se composent avec le mode vectoriel (`-vec`) ; les
  formes à état situées dans un domaine s'exécutent en temps de déclenchement
  sur tous les backends.
- Le compilateur Faust C++ est la référence pour la machinerie d'horloge
  elle-même, mais **il n'existe pas de référence C++ pour la combinaison de
  FAD/RAD avec les domaines d'horloge** — faust-rs en définit la sémantique, et
  l'oracle est l'accord numérique avec les différences finies.

Une définition référencée dans un corps est réinstanciée dans le domaine du
corps (section 3) : `ba.time` dans un bloc compte les déclenchements, et le
même `x @ n` écrit dans et hors d'un bloc fait deux lignes de retard. Une
entrée du bloc, ou un paramètre de lambda lié à l'extérieur, est échantillonné
au déclenchement. Les générateurs de tables sont l'exception par nature : une
table est remplie une fois, hors de tout domaine, où qu'elle soit lue.

Un `rad` dont la perte et les graines vivent dans un corps tourne dans le
domaine du bloc, à la cadence des trames : le synthétiseur harmonique de
[libraries/ddsp-examples-fr.md](../libraries/ddsp-examples-fr.md) (exemple
11) ajuste ainsi seize amplitudes par une perte spectrale calculée une fois
par trame.

## Voir aussi

- [fad-note-en.md](fad-note-en.md) — différentiation en mode direct
- [rad-usage-en.md](rad-usage-en.md) — différentiation en mode inverse
- [ondemand-fft-spectral-comparison-en.md](ondemand-fft-spectral-comparison-en.md) — traitement spectral bâti sur ces primitives
- `porting/ondemand-clock-domains-analysis-port-plan-2026-06-10-en.md` — sémantique côté compilateur et plan de portage
- `porting/ondemand-fad-rad-cohabitation-2026-06-10-en.md` — FAD/RAD × domaines, en détail
