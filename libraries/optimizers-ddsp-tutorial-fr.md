# Apprendre des paramètres DSP dans Faust : tutoriel pour débutant

Version anglaise :
[optimizers-ddsp-tutorial-en.md](optimizers-ddsp-tutorial-en.md) (même
contenu ; garder les deux versions synchronisées). Contexte et justification
des choix : [optimizers-overview-fr.md](optimizers-overview-fr.md).

Ce tutoriel suppose que vous savez lire et écrire du Faust ordinaire, et rien
d'autre. À la fin, vous aurez écrit des programmes qui apprennent un gain, le
pôle d'un filtre, la fréquence et le Q d'un filtre résonant et les cinq
coefficients d'un biquad, le tout dans le graphe Faust, et vous saurez vers
quel outil vous tourner quand quelque chose ne converge pas. Chaque programme a
été exécuté sur le compilateur courant ; les valeurs que vous devez observer
sont données après chacun.

## 0. Mise en place

Compiler avec le répertoire des bibliothèques locales au projet et celui des
bibliothèques standard de Faust sur le chemin d'import, et en double précision — les gradients des filtres récursifs perdent
vite en précision en simple précision :

```sh
faust-rs -double -I libraries -I <faustlibraries> -lang cpp programme.dsp
```

Pour *voir* un programme apprendre sans brancher d'audio, `faustprobe` le rend
hors ligne et imprime des trames choisies et des statistiques. Tous les
exemples ci-dessous ont été vérifiés avec lui ; remplacer `<faustlibraries>`
par le répertoire qui contient `stdfaust.lib` :

```sh
faustprobe --double -I libraries -I <faustlibraries> --in zero -n 3000 --every 500 programme.dsp
```

`-n` est le nombre de trames rendues, `--every` imprime une trame sur N,
`--quiet` n'imprime que les statistiques par sortie, `--in sine:220` injecte
une sinusoïde là où le programme a une entrée. Le reste est dans
[docs/faustprobe-user-guide-en.md](../docs/faustprobe-user-guide-en.md).

La bibliothèque se charge avec un préfixe :

```faust
op = library("optimizers.lib");
```

## 1. La plus petite boucle d'apprentissage, à la main

Commençons par un gain. Un système caché multiplie un signal par `0,7` ; nous
entendons sa sortie (la **cible**) et l'entrée, et nous voulons que notre propre
gain `g` s'y accorde.

Quatre idées, dans l'ordre où elles apparaissent dans le code :

- **modèle** : ce que nous calculons, `g * x` ;
- **perte** : à quel point nous nous trompons à cet échantillon,
  `(g * x - cible)^2` — au carré pour qu'elle soit positive et lisse ;
- **gradient** : comment la perte varie quand `g` varie. Pour cette perte,
  c'est `2 (g x - cible) x` : positif quand `g` est trop grand, négatif quand
  il est trop petit ;
- **mise à jour** : déplacer `g` à l'opposé du gradient,
  `g <- g - lr * gradient`, où la vitesse d'apprentissage `lr` fixe la taille
  du pas.

La mise à jour a besoin de mémoire : le nouveau `g` dépend du précédent. En
Faust, la mémoire est la récursion, et `~ _` renvoie la sortie précédente comme
entrée de l'échantillon suivant. Voici la boucle écrite à la main et, à côté,
la même boucle où `fad` calcule la dérivée à notre place :

```faust
import("stdfaust.lib");
x = no.noise;
target = 0.7 * x;
lr = 0.01;
loss(g) = (g * x - target) * (g * x - target);
// gradient écrit à la main : d/dg (g x - t)^2 = 2 (g x - t) x
g_manual = (\(g).(g - lr * 2.0 * (g * x - target) * x)) ~ _;
// le même gradient calculé par fad
g_fad = (\(g).(g - lr * (fad(loss(g), g) : !, _))) ~ _;
process = g_manual, g_fad, g_manual - g_fad;
```

`fad(loss(g), g)` renvoie deux signaux, la perte et sa dérivée par rapport à
`g` ; `: !, _` jette le premier et garde le second. La graine `g` est
l'argument de la lambda, c'est-à-dire la valeur précédente de la récursion.

Exécutez (`-n 1200 --every 200`). Les deux gains montent de 0 à `0,699` en
environ 1 000 échantillons (23 ms), et la troisième sortie — la différence
entre la dérivée manuelle et la dérivée automatique — vaut exactement `0` à
chaque trame. C'est toute la promesse de la différenciation automatique : la
dérivée de votre programme, exacte, sans l'écrire.

> **Pourquoi ça converge.** Le gradient pointe vers le haut de la perte ; faire
> un pas à l'opposé descend. Avec `lr = 0,01` et un bruit de variance unité, la
> constante de temps effective est d'environ `1 / (lr * E[x^2])` ≈ 300
> échantillons.

## 2. Lire `fad` et `rad`

Avant d'utiliser la bibliothèque, regardez ce que renvoient les primitives.
Deux sliders, un produit :

```faust
x = hslider("x", 2.0, 0.0, 10.0, 0.01);
y = hslider("y", 3.0, 0.0, 10.0, 0.01);
process = fad(x * y, (x, y)), rad(x * y, (x, y));
```

Exécutez avec `-n 1` : les six sorties sont `6, 3, 2, 6, 3, 2`.

- `fad(expr, (s0, s1))` donne chaque sortie d'`expr` suivie de ses dérivées
  par rapport à chaque graine : `[x*y, d/dx = y, d/dy = x]`.
- `rad(expr, (s0, s1))` donne toutes les sorties d'`expr`, puis les
  gradients : les mêmes nombres ici, dans une autre disposition.

Les graines sont les signaux que vous listez ; pour une perte à `N`
paramètres, un appel donne les `N` dérivées. L'essentiel de ce tutoriel
utilise `fad` ; `rad` revient à la section 4.1 (beaucoup de paramètres) et à la
section 10 (les hôtes).

## 3. La même boucle avec la bibliothèque

La bibliothèque empaquette la boucle de la section 1 sous le nom
`descend_1D` : on lui donne la perte comme fonction du paramètre, un **moteur**
qui transforme un gradient en pas, des bornes, une valeur initiale et un
signal de remise à zéro, et elle renvoie le paramètre appris :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.noise;
target = 0.7 * x;
loss(g) = op.mse(g * x, target);
g = op.descend_1D(loss, op.adam_g(0.002, 0.9, 0.999, 1e-8), -4.0, 4.0, 0.0, 0.0);
process = g, target - g * x;
```

Exécutez avec `-n 3000 --every 500` : `g` vaut `0,546, 0,685, 0,6998,
0,699999, 0,700000` à 500, 1 000, 1 500, 2 000, 2 500 échantillons, et la
seconde sortie, le résidu, tend vers zéro avec lui.

Trois choses ont changé par rapport à la section 1 :

- `op.mse(y, t)` est l'erreur quadratique ; la bibliothèque a d'autres pertes
  (section 7) ;
- `op.adam_g(lr, 0.9, 0.999, 1e-8)` est **Adam**, le moteur par défaut de
  l'apprentissage profond. Au lieu d'avancer de `lr * gradient`, il tient une
  moyenne courante du gradient (le moment) et de son carré, et avance de
  `lr * moyenne / sqrt(moyenne des carrés)` : la taille du pas est d'environ
  `lr` quelle que soit l'échelle du gradient. Là où la descente nue demande un
  `lr` réglé sur les unités du problème, Adam demande un `lr` réglé sur la
  vitesse à laquelle on veut bouger — ici 0,002 par échantillon ;
- les quatre derniers arguments sont les bornes `[-4, 4]`, la valeur initiale
  `0` et un signal de remise à zéro (`0` ici ; un `button("reset")` convient).

Les moteurs sont appliqués partiellement : `op.adam_g(0.002, 0.9, 0.999, 1e-8)`
est une fonction d'un argument restant, le gradient, avec lequel la boucle
l'appelle. Les autres moteurs de gradient ont la même forme : `op.sgd_g(lr)`,
`op.momentum_g(lr, 0.9)`, `op.rmsprop_g(lr, 0.999, 1e-8)`,
`op.lion_g(lr, 0.9, 0.99)`.

## 4. Un filtre : moindres carrés et normalisation

Maintenant un paramètre à l'intérieur d'une récursion : le pôle d'un filtre à
un pôle `y[n] = x[n] + p y[n-1]`. Deux nouveautés. Le modèle a de la mémoire,
donc la dérivée de sa sortie par rapport à `p` dépend de tout le passé — `fad`
s'en charge en transportant la dérivée avec l'état, vous n'avez pas à y penser.
Et nous passons à la seconde famille de boucles de la bibliothèque, `lsq_1D`,
qui différencie le **modèle** plutôt que la perte :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
onepole(p, sig) = sig : + ~ *(p);
x = no.noise;
target = onepole(0.6, x);
p = op.lsq_1D(onepole, op.nlms(0.01, 1e-6, 0.99), -0.99, 0.99, 0.0, 0.0, target, x);
process = p, target - onepole(p, x);
```

`lsq_1D(mdl, moteur, lo, hi, init, reset, cible, x)` prend le modèle comme
fonction `mdl(p, x)`, et la cible et l'entrée comme signaux ; la perte est
l'erreur quadratique. Ses moteurs reçoivent deux nombres au lieu d'un : le
résidu `r = modèle - cible` et la **sensibilité** `j = d(modèle)/dp`. Les
garder séparés permet **NLMS**, le LMS normalisé : le pas `mu * r * j` est
divisé par la puissance lissée de `j`, donc il ne dépend pas du niveau de
l'entrée.

Exécutez avec `-n 4000 --every 500` : `p` vaut `0,600013` à 500 échantillons et
`0,600000` à partir de 2 000.

Pourquoi la normalisation compte : avec un pas `op.lms(0.02)` nu réglé pour une
entrée de niveau 1, le même FIR à 3 coefficients converge parfaitement au
niveau 1, est cent fois trop lent au niveau 0,1 et bute sur ses bornes au
niveau 10 ; avec `op.nlms(0.02, 1e-6, 0.99)` il converge à l'identique aux
trois niveaux. Les niveaux audio varient de 40 dB dans une session :
normalisez.

### 4.1 Beaucoup de coefficients : boucles à bus et mode inverse

`lsq_3D` prend trois coefficients comme trois arguments, chacun avec son
moteur et ses bornes. Pour seize coefficients, la bibliothèque porte les
paramètres par un *bus* et applique un moteur et une paire de bornes à tous :
`lsq_N`, et son pendant perte d'abord `descend_N`. Le modèle devient un bloc
dont les `N` premières entrées sont les coefficients :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
N = 16;
x = no.noise;
taps = x <: par(i, N, @(i));
fir(h) = (h, taps) : ro.interleave(N, 2) : par(i, N, *) :> _;
h_star(i) = sin(0.5 * i) * exp(-0.2 * i);
target = fir(par(i, N, h_star(i)));
fir_loss = fir(si.bus(N)) : sq_err with { sq_err(y) = op.mse(y, target); };
h = op.descend_N_rad(N, fir_loss, op.sgd_g(0.02), -2.0, 2.0, 0.0, 0.0);
process = target - fir(h);
```

`descend_N_rad` est `descend_N` avec `rad` à la place de `fad` : un balayage
inverse par échantillon donne les seize gradients, là où le mode direct
transporte seize tangentes. Exécutez les deux (`op.descend_N` est l'autre) :
les résidus sont le même signal à l'arrondi près, et les programmes compilés
ne le sont pas — 1 182 instructions d'interpréteur contre 3 777, 0,04 s
contre 0,10 s pour 200 000 échantillons ; 4 129 contre 28 891 et 0,13 s
contre 1,32 s à 64 coefficients. La sensibilité d'un coefficient de FIR est
son entrée retardée, donc les deux boucles calculent le même gradient. Là où
elles diffèrent, c'est un modèle avec une récursion entre les paramètres et la
sortie : un gradient consommé dans une boucle ne peut pas attendre la fin du
bloc, donc `rad` ne voit qu'un échantillon et renvoie le *terme direct*,
l'état passé tenu fixe (`2 r y[n-1]` pour le filtre à un pôle ci-dessus), là
où `fad` transporte la dérivée à travers la récursion. Le terme direct est le
gradient de la régression pseudo-linéaire du filtrage adaptatif IIR, moins
cher et convergent sous une condition de positivité sur le modèle ; celui de
`fad` est le gradient de l'erreur de prédiction récursive, la direction de
descente exacte (section 4.7 de la synthèse). La règle : beaucoup de
paramètres et un modèle sans récursion, `_rad` ; une récursion à apprendre,
`fad`.

Deux détails du programme. `fir(si.bus(N))` est le modèle appliqué à seize
entrées ouvertes, le bloc qu'attend une boucle à bus. Et la perte nomme son
entrée (`sq_err(y)`) au lieu d'écrire `op.mse(_, target)` : un `_` libre est
dupliqué partout où l'argument est utilisé, donc `(_ - t) * (_ - t)` serait
un bloc à deux entrées et `:>` répartirait les coefficients entre elles — la
boucle n'apprend alors rien, et rien ne prévient.

## 5. Deux paramètres d'unités différentes

Identifions un passe-bas résonant : cible `fi.resonlp(1200, 2.0)`, modèle
`fi.resonlp(f, q)`, départ à `(1000, 1.0)`. La fréquence est en hertz, le
facteur de qualité est sans unité. C'est le moment où les débutants perdent une
journée ; regardez ce qui se passe avec une seule vitesse d'apprentissage.

### 5.1 Une vitesse pour les deux : le problème d'échelle

Lion est un moteur qui avance d'exactement `±lr` dans la direction du signe de
son moment, quelle que soit l'amplitude du gradient — un bon défaut quand les
paramètres ont des unités différentes, à condition que `lr` ait un sens pour
chacun :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.noise;
target = x : fi.resonlp(1200.0, 2.0, 1.0);
loss(f, q) = op.mse(x : fi.resonlp(f, q, 1.0), target);
lion = op.lion_g(0.001, 0.9, 0.99);
learned = op.descend_2D(loss, lion, lion, 20.0, 20000.0, 0.1, 10.0, 1000.0, 1.0, 0.0);
process = learned;
```

Exécutez avec `-n 100000 --every 10000` : `q` atteint 2,0, mais `f` bouge de
0,001 Hz par échantillon — 1 Hz tous les 1 000 échantillons — et en est encore
à 1 090 Hz après 100 000 échantillons. Un pas correct pour `q` est absurdement
petit pour `f`. Deux remèdes suivent ; les deux valent d'être connus.

### 5.2 Premier remède : apprendre dans un domaine où les pas ont un sens

Apprendre `u = log(f)` au lieu de `f`. Un pas de 0,001 sur `u` est une
variation de fréquence de 0,1 %, une quantité de même nature qu'un pas de 0,001
sur `q`. Le modèle applique simplement `exp` :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.noise;
target = x : fi.resonlp(1200.0, 2.0, 1.0);
loss(u, q) = op.mse(x : fi.resonlp(exp(u), q, 1.0), target);
lion = op.lion_g(op.lr_exp(0.001, 0.00001, 20000.0), 0.9, 0.99);
learned = op.descend_2D(loss, lion, lion, log(20.0), log(20000.0), 0.1, 10.0, log(1000.0), 1.0, 0.0);
u = learned : _, !;
q = learned : !, _;
process = exp(u), q;
```

`op.lr_exp(0.001, 0.00001, 20000)` est un **schedule** de vitesse
d'apprentissage : elle décroît exponentiellement de 0,001 vers 0,00001 avec une
constante de temps de 20 000 échantillons, de sorte que la recherche est rapide
au début et calme à la fin. Les vitesses d'apprentissage sont des signaux ; un
schedule se passe là où on mettrait une constante.

Exécutez : `(1206, 2,003)` à 10 000 échantillons, puis à environ 2 % de
`(1200, 2,0)`. Bien, avec une gigue résiduelle que laisse le pas fixe de Lion.

### 5.3 Second remède : laisser l'algorithme trouver les échelles

`lm_2D` est une boucle de **Gauss-Newton** amortie, la forme récursive de la
méthode que l'identification de systèmes utilise depuis des décennies. Elle
construit une matrice 2×2 à partir des sensibilités des deux paramètres, qui
encode à quel point chacun affecte la sortie et comment ils interagissent, et
résout pour obtenir le pas. Pas de vitesse par paramètre : un gain `mu`, un
amortissement `lambda`, un facteur d'oubli `a` :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.noise;
target = x : fi.resonlp(1200.0, 2.0, 1.0);
mdl(f, q, sig) = sig : fi.resonlp(f, q, 1.0);
process = op.lm_2D(mdl, 0.01, 0.1, 0.99, 20.0, 20000.0, 0.1, 10.0, 1000.0, 1.0, 0.0, target, x);
```

Exécutez avec `-n 30000 --every 5000` : `(1200,000000, 2,000000)` à 5 000
échantillons, et ça y reste. Règle pratique pour ses réglages : `mu = 1 - a`,
`lambda = 0,1`, `a` entre 0,99 et 0,999. `lm_3D` fait de même pour trois
paramètres.

## 6. Stabilité : le biquad à cinq coefficients

Un biquad a trois coefficients de zéros `b0, b1, b2` et deux coefficients de
pôles `a1, a2`. Les pôles sont dangereux : hors de la région `|a2| < 1`,
`|a1| < 1 + a2` le filtre est instable, et une fois qu'il a explosé aucun
optimiseur ne s'en remet. Borner `a1` et `a2` à un rectangle ne suffit pas,
parce que la région stable est un triangle. Essayez : apprenez `(a1, a2)`
directement avec des bornes rectangulaires, en partant de `(1,9, -0,5)` — dans
le rectangle, hors du triangle — et, côte à côte, apprenez deux **coefficients
de réflexion** `k1, k2 dans (-1, 1)` que la bibliothèque transforme en
`a1 = k1 (1 + k2)`, `a2 = k2`. Cette transformation couvre exactement le
triangle : tout point de la boîte `(k1, k2)` est un filtre stable :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.pink_noise;
target = fi.tf2(0.1, 0.2, 0.1, -1.0, 0.4, x);
adam = op.adam_g(0.001, 0.9, 0.999, 1e-8);
// (a) a1, a2 appris directement, bornes rectangulaires, départ dans les bornes mais hors du triangle de stabilité
loss_raw(b0, b1, b2, a1, a2) = op.mse(fi.tf2(b0, b1, b2, a1, a2, x), target);
raw = op.descend_5D(loss_raw, adam, adam, adam, adam, adam,
                    -2, 2, -2, 2, -2, 2, -1.92, 1.92, -0.92, 0.92,
                    0, 0, 0, 1.9, -0.5, 0);
// (b) coefficients de réflexion : tout point de la boîte est un filtre stable
loss_k(b0, b1, b2, k1, k2) = op.mse(fi.tf2(b0, b1, b2, a1, a2, x), target)
with { a1 = op.poles_from_reflection(k1, k2) : _, !; a2 = op.poles_from_reflection(k1, k2) : !, _; };
kk = op.descend_5D(loss_k, adam, adam, adam, adam, adam,
                   -2, 2, -2, 2, -2, 2, -0.999, 0.999, -0.999, 0.999,
                   0, 0, 0, 0.95, -0.5, 0);
r4 = raw : !, !, !, _, !;
k1 = kk : !, !, !, _, !;
k2 = kk : !, !, !, !, _;
process = r4, op.poles_from_reflection(k1, k2);
```

Exécutez avec `-n 200000 --every 20000` : le `a1` brut (première sortie) est
collé à sa borne `1,92` dès les premières trames — le filtre a explosé et le
gradient n'a plus de sens — tandis que la forme en réflexion (deuxième et
troisième sorties) atteint `(-0,999, 0,3999)` pour une cible de `(-1,0, 0,4)`
après 60 000 échantillons.

Notez les deux idiomes Faust dans `loss_k` : il n'y a pas de destructuration,
donc les deux sorties de `poles_from_reflection` sont projetées avec `: _, !`
et `: !, _` ; et une expression à cinq sorties appliquée à une fonction à cinq
arguments est une application partielle, pas un étalement, donc les
coefficients sont projetés un par un.

L'exemple complet, avec une cible contrôlée par l'utilisateur, un bouton de
remise à zéro et une seule vitesse Lion sur un schedule exponentiel, est la
section 4 de [docs/fad-rad-synthesis-fr.md](../docs/fad-rad-synthesis-fr.md) :
les cinq coefficients arrivent à moins de `1e-5` de la cible après 300 000
échantillons.

## 7. La perte vous appartient

Avec `descend_ND`, la perte est n'importe quelle fonction Faust des
paramètres. Deux situations où l'erreur quadratique est la mauvaise perte :

### 7.1 Valeurs aberrantes

Ajoutez des impulsions de `±20` tous les 97 échantillons à une cible
d'amplitude 0,7, et apprenez le gain à travers `mse` et à travers `logcosh`,
une perte quadratique près de zéro et linéaire au loin, dont le gradient est
donc borné :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
x = no.noise;
spike = float((ba.time % 97) == 0) * 20.0 * op.sgn(x');
target = 0.7 * x + spike;
g_mse = op.descend_1D(\(g).(op.mse(g * x, target)), op.sgd_g(0.01), -4, 4, 0, 0);
g_robust = op.descend_1D(\(g).(op.logcosh(g * x, target)), op.sgd_g(0.01), -4, 4, 0, 0);
process = g_mse, g_robust;
```

Exécutez avec `-n 40000 --every 5000` : le gain `mse` erre entre 0,49 et 0,88,
secoué par chaque impulsion ; le gain `logcosh` reste dans 0,69–0,71.
`op.pseudo_huber(delta, y, t)` est l'autre perte robuste, avec une échelle de
transition explicite.

### 7.2 Apparier un son, pas une forme d'onde

L'erreur échantillon par échantillon suppose que le modèle et la cible voient
la *même* excitation. Souvent ce n'est pas le cas : on veut que le modèle
*sonne* comme la cible, pas qu'il reproduise sa forme d'onde. Comparer des
puissances lissées ignore la phase. Ici la cible est un passe-bas à 800 Hz sur
un bruit, le modèle un passe-bas sur un bruit indépendant, la perte compare les
puissances en log, et la coupure est apprise en domaine log avec un SGD nu :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
excitation_a = no.noise;
excitation_b = no.noises(4, 1);   // un générateur de bruit indépendant
target = excitation_a : fi.lowpass(1, 800.0);
loss(u) = op.log_energy_loss(0.999, 1e-9, excitation_b : fi.lowpass(1, exp(u)), target);
u = op.descend_1D(loss, op.sgd_g(0.00005), log(20.0), log(10000.0), log(3000.0), 0.0);
process = exp(u), exp(op.polyak(0.9999, u));
```

Exécutez avec `-n 400000 --every 50000` : la coupure descend de 3 000 Hz et se
stabilise entre 770 et 850 Hz — une gigue qui vient de l'estimation d'une
puissance sur environ 1 000 échantillons de bruit. `op.polyak(0.9999, u)` est
une lecture lissée du paramètre pour le chemin audible. Avec `mse` sur cette
paire de signaux, la coupure resterait bloquée sur la borne de 20 Hz : il n'y a
rien à apprendre d'une forme d'onde qu'on ne peut pas reproduire.

Deux règles que cet exemple enseigne :

- une perte bâtie sur un lissage (`energy_loss`, `log_energy_loss`) voit le
  monde avec un retard d'environ `1 / (1 - a)` échantillons ; l'optimiseur doit
  être plus lent que cela, sinon la boucle oscille — d'où `lr = 5e-5` ici ;
- sur une perte bruitée, préférez le SGD à Adam : le pas du SGD suit
  l'amplitude du gradient et s'éteint près de l'optimum, celui d'Adam vaut
  toujours environ `lr`, ce qui devient une marche aléatoire.

## 8. Hygiène : schedules, gating, lecture, remise à zéro

Les signaux réels s'arrêtent et repartent. Un optimiseur qui continue
d'apprendre dans le silence dérive sur le bruit ; un qui ne ralentit jamais
gigue indéfiniment. Cet exemple assemble les outils : le signal est présent la
moitié du temps, un petit bruit de mesure est ajouté, l'apprentissage est
conditionné à la puissance d'entrée, la vitesse décroît, la lecture est
moyennée, et un bouton remet le paramètre à zéro :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
present = float((ba.time % 20000) < 10000);        // signal présent la moitié du temps
x = no.noise * present;
target = 0.7 * x + 0.001 * no.noises(4, 2);       // plus un bruit de mesure
loss(g) = op.mse(g * x, target);
vad = op.ema(0.99, x * x) > 0.001;                 // n'apprendre que s'il y a du signal
lr = op.lr_exp(0.02, 0.001, 20000.0);
upd(grad) = op.sgd_g(lr, op.gate_g(vad, grad));
g = op.descend_1D(loss, upd, -4, 4, 0, button("reset"));
process = g, op.polyak(0.999, g), lr;
```

Exécutez avec `-n 80000 --every 10000` : `g` vaut `0,69994` après 10 000
échantillons et reste à `±5e-5` de 0,7 ensuite ; la troisième sortie montre la
vitesse passer de 0,02 à 0,0016. `upd` montre comment le conditionnement se
compose : c'est une fonction ordinaire du gradient, bâtie avec des briques de
la bibliothèque, passée comme moteur.

`gate_g` met le gradient à zéro mais le calcule quand même, et le modèle
qui porte les tangentes tourne aussi. Pour cesser de payer l'apprentissage
une fois les paramètres stabilisés, mettre toute la boucle dans un
`ondemand` dont un critère de convergence coupe l'horloge : la section 7 de
l'aperçu montre le motif et son coût, un facteur huit à vingt-cinq.

## 9. Résoudre plutôt qu'apprendre : Newton

La même mécanique de dérivée résout des équations. Les modèles analogiques
virtuels en sont pleins d'implicites — la sortie d'une boucle de rétroaction
saturante dépend d'elle-même : `y = tanh(x - fb * y)`. La méthode de Newton
trouve `y` en quelques pas, chacun demandant le résidu
`F(y) = y - tanh(x - fb y)` et sa dérivée `F'(y)` ; un `fad` donne les deux, et
`op.newton(N, F, y0)` déroule `N` pas :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
fb = 2.0;
// saturateur implicite : y = tanh(x - fb * y), résolu en y à chaque échantillon
residual(x, y) = y - ma.tanh(x - fb * y);
solve(x) = op.newton(5, residual(x), 0.0);
process(x) = solve(x), residual(x, solve(x));
```

Exécutez avec `--in sine:220 -n 2000 --skip 1000 --quiet` : la première sortie
est le signal résolu (crête 0,33 pour une sinusoïde unité, la rétroaction le
compresse), la seconde est le résidu de la solution — `0` à la précision
numérique à chaque trame. C'est la brique de base des filtres à rétroaction
sans délai et des écrêteurs à diodes.

## 10. Remettre les gradients à un hôte : `rad`

Jusqu'ici tout apprenait dans le graphe. Parfois le programme doit seulement
*produire* des dérivées, et un hôte (un plugin, un script Python, un banc de
test) se charge de l'accumulation et de la mise à jour — par exemple pour
entraîner sur un lot d'enregistrements plutôt que sur le signal vivant. C'est
à cela que sert `rad` :

```faust
gain = hslider("gain", 1.0, -4.0, 4.0, 0.001);
bias = hslider("bias", 0.0, -4.0, 4.0, 0.001);
process = rad(gain * _ + bias, (gain, bias));
```

Exécutez avec `--in sine:220 -n 5` : trois sorties, `[gain * x + bias, x, 1]`
— la sortie et ses deux gradients. L'hôte les lit, forme le gradient de la
perte (`2 * (out - cible) * d/dgain`, sommé sur un bloc), et réécrit les
sliders. [docs/rad-usage-en.md](../docs/rad-usage-en.md) donne la boucle
complète en Rust, y compris un filtre coupe-bande adaptatif. En sortie
publique, `rad` travaille bloc par bloc à travers les délais et les
récursions : le balayage remonte le bloc `compute` courant, la dérivée remise
à zéro à sa fin, et les voies de gradient sont des contributions par
échantillon que l'hôte somme. Consommé dans le graphe, comme à la section
4.1, il ne voit qu'un échantillon.

## 11. Apprendre à sa propre cadence : `ondemand`

Jusqu'ici tout tournait une fois par échantillon : le modèle, la dérivée et la
mise à jour. Rien n'oblige la mise à jour à être aussi fréquente. `faust-rs` a
une primitive, `ondemand`, qui n'exécute une sous-expression que lorsqu'une
horloge tire et maintient ses sorties entre deux :

```faust
(horloge, entrées...) : ondemand(corps)
```

`ondemand(corps)` a une entrée de plus que `corps`, l'horloge, en premier. Dans
le corps, le temps est le *temps de tir* : une récursion `~` avance une fois
par tir, un délai dure un tir. C'est exactement ce que veut un optimiseur qui
doit faire un pas par trame. `interleave.lib` (aussi dans `libraries/`)
fournit l'horloge de trame, `il.frame_clock(N)`, qui tire tous les `N`
échantillons, et `il.serialize_in(N)`, qui transforme un flux en les `N`
échantillons parallèles de la trame courante.

### 11.1 Tout l'optimiseur, cadencé

Mettez la boucle de la section 3 dans un bloc qui tire tous les 64
échantillons. Le corps reçoit l'excitation et la cible en entrées et ferme la
perte sur elles :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
il = library("interleave.lib");
x = no.noise;
target = 0.7 * x;
learn(xi, ti) = op.descend_1D(\(g).(op.mse(g * xi, ti)), op.adam_g(0.02, 0.9, 0.999, 1e-8), -4.0, 4.0, 0.0, 0.0);
g = (il.frame_clock(64), x, target) : ondemand(learn);
process = g, target - g * x;
```

Exécutez avec `-n 20000 --every 2000` : `g` vaut `0,630` à 4 000 échantillons,
`0,7097` à 6 000, `0,70005` à 12 000 et `0,7000 ± 1e-6` à la fin — après 312
pas d'optimiseur au lieu de 20 000. Entre deux tirs `g` est maintenu, donc le
modèle `g * x` à cadence audio voit toujours un paramètre valide. Le graphe
`fad`, la partie coûteuse, tourne 64 fois moins souvent ; le prix est que
chaque pas ne voit qu'un échantillon de la trame, d'où `lr = 0,02` plutôt que
`0,002`.

### 11.2 Gradient à cadence audio, mise à jour par trame

Un meilleur usage de la trame : calculer le gradient à chaque échantillon, le
moyenner, et laisser le bloc appliquer un pas par trame. Le bloc reçoit le
paramètre précédent et le gradient moyenné en entrées explicites, et sa sortie
maintenue est rebouclée par `~` :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
il = library("interleave.lib");
N = 64;
x = no.noise;
target = 0.7 * x;
lr = 0.5;
g = learn ~ _
with {
    learn(gprev) = (il.frame_clock(N), gprev, gavg) : ondemand(step)
    with {
        grad = fad(op.mse(gprev * x, target), gprev) : !, _;   // cadence audio
        gavg = op.ema(1.0 - 1.0 / N, grad);                     // moyenné sur ~N échantillons
        step(gp, ga) = op.clip(-4.0, 4.0, gp - lr * ga);        // une fois par trame
    };
};
process = g, target - g * x;
```

Exécutez avec `-n 20000 --every 2000` : `0,700000` dès 4 000 échantillons. Rien
n'est perdu à moyenner : le pas par trame voit le gradient moyen de la trame,
ce que voit un pas par lot dans un framework d'entraînement. Notez la forme :
la graine `gprev` et le `fad` sont hors du bloc, la mise à jour dedans, et les
deux ne communiquent que par les entrées du bloc et sa sortie maintenue.

La bibliothèque empaquette ce motif sous le nom `descend_1D_clocked` (et
`2D` … `5D`) : mêmes arguments que `descend_1D`, l'horloge en premier. Elle
moyenne le gradient avec `op.frame_mean`, une moyenne exacte sur la trame
remise à zéro par l'horloge, garde le paramètre à `init` jusqu'au premier tir,
et verrouille un reset plus court qu'une trame jusqu'au tir suivant :

```faust
import("stdfaust.lib");
op = library("optimizers.lib");
il = library("interleave.lib");
x = no.noise;
target = 0.7 * x;
loss(g) = op.mse(g * x, target);
g = op.descend_1D_clocked(il.frame_clock(64), loss, op.sgd_g(0.5), -4.0, 4.0, 0.0, 0.0);
process = g, target - g * x;
```

Exécutez avec `-n 20000 --every 2000` : `0,700000` dès 4 000 échantillons, comme
la version écrite à la main. Le moteur tourne en temps de tir, donc un Adam ou
un schedule donné ici compte des trames, pas des échantillons : sur le filtre
résonant de la section 5, `op.descend_2D_clocked(il.frame_clock(64), loss, adam, adam, ...)`
avec `adam = op.adam_g(0.02, 0.9, 0.999, 1e-8)` sur `(log f, q)` atteint
`(1200,2, 1,996)` après 10 000 échantillons et reste ensuite à moins de 1 % de
`(1200, 2,0)` — le pas fixe d'Adam laisse la petite oscillation habituelle,
qu'un schedule supprime.

### 11.3 Une perte spectrale, un pas par trame

C'est le motif qui rapproche le plus Faust de la façon dont les articles DDSP
entraînent : une perte sur un *spectre*, mise à jour une fois par trame. La
trame de `N = 8` échantillons entre dans le bloc par huit entrées ; à
l'intérieur, la perte met la trame à l'échelle par `g`, prend une FFT (`an.fft`
d'`analyzers.lib`), somme les modules et compare la somme à une cible ;
`descend_1D` tourne sur cette perte, en temps de tir :

```faust
il = library("interleave.lib");
an = library("analyzers.lib");
si = library("signals.lib");
no = library("noises.lib");
op = library("optimizers.lib");
N = 8;
target_energy = 4.0;
cmag(re, im) = sqrt(re * re + im * im + 0.000000001);
magsum = par(m, N, cmag) :> _;
// Le bloc reçoit les N échantillons de la trame comme arguments nommés (un
// opérateur de trame à entrées `_` libres verrait ses entrées dupliquées à chaque usage).
learn(x0, x1, x2, x3, x4, x5, x6, x7) =
    op.descend_1D(loss, op.adam_g(0.02, 0.9, 0.999, 1e-8), 0.01, 10.0, 1.0, 0.0)
with {
    // perte spectrale de la trame mise à l'échelle par g : (somme |X_k| - cible)^2
    loss(g) = (x0, x1, x2, x3, x4, x5, x6, x7) : par(i, N, *(g) : (_, 0)) : an.fft(N) : magsum : -(target_energy) <: _ * _;
};
process = no.noise : il.serialize_in(N) : (il.frame_clock(N), si.bus(N)) : ondemand(learn);
```

Exécutez avec `-n 40000 --skip 20000 --quiet` : `g` vaut en moyenne `0,3405` sur
la seconde moitié, avec une gigue de trame à trame d'environ `±0,03` — le
spectre d'une trame aléatoire varie, donc la perte est bruitée. L'optimum des
moindres carrés pour ce bruit se calcule à la main : `0,340`. Un module avec un
epsilon sous la racine garde la dérivée définie sur un bin vide.

Le commentaire dans le code est une règle à retenir : un opérateur de trame
écrit avec des entrées `_` libres ne doit pas circuler comme expression
ouverte, parce que chaque usage duplique ses entrées — l'arité du bloc explose
en « sequential composition mismatch ». Donnez au corps des arguments nommés.

Une variante garde le paramètre hors du bloc et le passe en entrée explicite ;
le bloc sort alors le gradient maintenu, et la mise à jour
`g - lr * grad * horloge` est conditionnée par l'horloge à cadence audio. Elle
atteint le même `0,34`. Ce qui ne marche *pas*, c'est de lire un signal audio
extérieur dans le corps par son nom : une définition référencée dans un corps
est instanciée à nouveau dans le temps propre du corps (`ba.time` dans un bloc
compte les déclenchements, un oscillateur extérieur devient un nouvel
oscillateur qui avance d'un pas par déclenchement), si bien que le signal
extérieur n'est vu qu'à travers une entrée. Gardez la graine, la perte et la
mise à jour dans le même domaine, ou reliez-les par les entrées du bloc.

Trois dernières choses sur les domaines d'horloge. `ma.SR` n'est pas adapté
dans `ondemand` (sa cadence est inconnue statiquement) : calculez les valeurs
qui dépendent de la cadence à l'extérieur et passez-les en entrée. `rad` ne
traverse pas une frontière de domaine, mais les boucles à bus ont des versions
`_rad` cadencées (`descend_N_rad_clocked`) : le balayage inverse tourne à
cadence audio dans la trame, seul le pas est cadencé ; et un `rad` dont la
perte et les graines vivent dans le bloc y tourne à la cadence des trames
(exemple 11 de [ddsp-examples-fr.md](ddsp-examples-fr.md)). Et la référence pour les
primitives elles-mêmes, `upsampling` et `downsampling` compris, est
[docs/ondemand-note-fr.md](../docs/ondemand-note-fr.md).

## 12. Pour aller plus loin

- **Effets adaptatifs.** La section 6 de
  [docs/fad-rad-synthesis-fr.md](../docs/fad-rad-synthesis-fr.md) est une
  boucle de contrôle actif du bruit (FxLMS) écrite avec `fad` ; le fichier du
  corpus `tests/corpus/auto_wah_fad_host.dsp` est un auto-wah dont les
  gradients sont exposés à l'hôte.
- **Pertes spectrales.** `tests/corpus/ondemand_fad_spectral_loss_008.dsp`
  différencie une perte calculée sur une trame FFT, le pendant par trame de la
  section 7.2.
- **Exemples complets.** [ddsp-examples-fr.md](ddsp-examples-fr.md) : onze
  programmes DDSP avec leurs tests — un notch adaptatif, un mode calibré par
  Gauss-Newton, un modèle d'ampli, un diode clipper appris à travers son
  solveur implicite, une réverbération FDN, une corde accordée à travers son
  retard fractionnaire (`fad`) ; un annuleur d'écho, un waveshaper neuronal,
  des gradients par bloc pour un hôte, un ampli GRU entraîné par BPTT par
  blocs, un synthétiseur harmonique ajusté par une perte spectrale dans un
  bloc `ondemand` (`rad`).
- **Beaucoup de paramètres.** `tests/corpus/opt_descend_n_rad_fir16.dsp` et
  `tests/corpus/opt_lsq_n_rad_nlms_fir8.dsp` sont les boucles à bus sur des
  FIR ; `tests/corpus/opt_bus_fad_vs_rad_fir16.dsp` fait tourner côte à côte
  la version directe et la version inverse.
- **Mode inverse et hôtes.** [docs/rad-note-en.md](../docs/rad-note-en.md)
  pour l'algorithme, [docs/rad-usage-en.md](../docs/rad-usage-en.md) pour le
  flux de travail.
- **La bibliothèque elle-même.** Chaque fonction d'[optimizers.lib](optimizers.lib)
  porte un exemple `#### Test` compilé par la suite de tests ; ce sont les
  plus petits usages fonctionnels de chaque fonction.

## 13. Murs fréquents

| Symptôme | Cause probable | Remède |
|---|---|---|
| Le paramètre ne bouge jamais | sa dérivée est nulle : il traverse un bouton, une case à cocher, une conversion ou une comparaison entière dans le modèle | garder le chemin du paramètre en arithmétique flottante |
| Il bouge dans le mauvais sens | convention de signe : avec `r = modèle - cible` le gradient MSE est `+2 r j` ; la note de synthèse utilise `err = cible - modèle` et `-err * j` | choisir une convention |
| `NaN` au bout d'un moment | `abs` (dérivée `x/|x|`) ou un filtre devenu instable | pertes lisses (`logcosh`, `pseudo_huber`), coefficients de réflexion pour les pôles |
| Un paramètre converge, un autre rampe | unités différentes sous une seule vitesse | domaine log, Adam/Lion, ou `lm_2D` |
| La boucle oscille avec une perte énergétique | l'optimiseur est plus rapide que le lissage de la perte | baisser `lr` sous `1 - a` |
| Gigue à la fin | pas fixe sur un gradient bruité | `lr_exp`/`lr_cos`, `polyak`, ou SGD au lieu d'Adam |
| `(a, b) = f(...)` ne parse pas | Faust n'a pas de destructuration | `a = f(...) : _, !; b = f(...) : !, _;` |
| `mdl(opts)` a la mauvaise arité | une expression multi-sorties est un seul argument | projeter chaque sortie et les passer séparément |
| Converge en double mais pas en simple précision | perte de précision dans les tangentes récursives | compiler avec `-double` |
| `sequential composition mismatch` autour d'un bloc `ondemand` | un opérateur de trame à entrées `_` libres utilisé plusieurs fois | donner au corps des arguments nommés, un par échantillon de la trame |
| Un bloc ignore ce qui se passe dehors | une définition référencée dans le corps est instanciée à nouveau dans le temps du corps, ce n'est pas le signal extérieur | passer les signaux extérieurs en entrées explicites du bloc |
| Une boucle à bus n'apprend rien, les coefficients errent autour de zéro | `op.mse(_, t)` (toute fonction appliquée à un `_` libre) est un bloc à deux entrées : `:>` répartit les coefficients entre elles | nommer l'entrée de la perte : `\(y).(op.mse(y, t))` |
| Une boucle `_rad` converge moins vite que la boucle `fad` sur un modèle récursif | dans une boucle, `rad` renvoie le terme direct, l'état passé tenu fixe | les boucles `fad` pour les modèles récursifs, les unes ou les autres pour les modèles sans récursion |
| La pente `fad` d'un solveur implicite manque d'un terme | l'itération part de `vprev`, le signal même que l'équation tient fixe : `fad(G(vprev, v), v)` avec `v = vprev` dérive les deux | partir d'un prédicteur ou de tout signal distinct |

## Glossaire

- **Modèle** : l'expression Faust dont les paramètres sont appris.
- **Cible** : le signal que le modèle devrait produire.
- **Perte** : une mesure scalaire de l'erreur à l'échantillon courant.
- **Gradient** : la dérivée de la perte par rapport aux paramètres ;
  **sensibilité** (`j`) : la dérivée de la sortie du modèle.
- **Graine** : le signal par rapport auquel `fad` ou `rad` dérive.
- **Tangente** : une dérivée produite par l'AD en mode direct (`fad`).
- **Terme direct** : la dérivée de la sortie d'un modèle récursif par rapport
  à un paramètre, son état passé tenu fixe ; ce que `rad` renvoie dans une
  boucle, et le gradient de la régression pseudo-linéaire du filtrage
  adaptatif.
- **Moteur** : la fonction qui transforme un gradient (ou un résidu et une
  sensibilité) en pas.
- **Vitesse d'apprentissage** (`lr`) : la taille du pas ; un **schedule** la
  fait varier.
- **Résidu** (`r`) : `modèle - cible`.
- **Reparamétrisation** : apprendre un paramètre transformé (un log, un
  coefficient de réflexion) pour que toute valeur soit admissible.
