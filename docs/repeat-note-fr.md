---
title: "Note : la primitive repeat, et les primitives à horloge construites dessus"
author: "Stéphane Letz et Claude Fable 5.1"
date: "2026-09-19"
---

# `repeat` : une boucle bornée avec sortie, et `ondemand` / `upsampling` / `downsampling` comme formes dérivées

Version anglaise : [repeat-note-en.md](repeat-note-en.md) (même contenu ; garder les deux versions synchronisées à chaque amendement).

Cette note énonce la sémantique de `repeat`, une primitive à horloge proposée le 2026-09-18, et montre comment les trois primitives de domaine d'horloge de faust-rs, `ondemand`, `upsampling` et `downsampling`, en sont des formes dérivées. Elle est écrite du point de vue du programmeur Faust et ne dit rien de la façon dont le compilateur réalise tout cela ; c'est l'objet de `porting/repeat-counted-loop-with-exit-analysis-and-plan-2026-09-18-en.md`. Les trois primitives elles-mêmes sont présentées dans [ondemand-note-fr.md](ondemand-note-fr.md), que cette note suppose lue.

Tous les nombres cités ont été mesurés le 2026-09-18 avec `faustprobe`, en double précision à 48 kHz, sur la forme de bibliothèque de `repeat` écrite avec les primitives existantes (le plan, section 1.5) ; la sémantique ci-dessous n'est donc pas un dessin sur le papier mais un comportement qui tourne.

## 1. Pourquoi `repeat`

### 1.1 Les boucles que Faust possède

Faust a trois façons de répéter un calcul, et aucune ne fait tourner une boucle dont la fin est décidée par la boucle :

- `par`, `seq`, `sum` et `prod` se déroulent à la compilation. Le compte est un littéral, le graphe grandit avec lui, rien n'est décidé à l'exécution.
- `~` fait un pas par échantillon. Une récursion est une boucle dont les itérations sont les échantillons ; elle ne peut pas faire deux pas dans un échantillon.
- un bloc `ondemand` à horloge entière fait tourner son corps `H` fois par échantillon, `H` étant un signal : une boucle avec état à l'exécution, la première que Faust ait eue. Mais `H` est un signal du domaine extérieur, calculé avant que le bloc ne tourne. Le compte peut dépendre des entrées, de l'état maintenu, de la solution précédente ; il ne peut pas dépendre de ce que les itérations calculent.

### 1.2 Les boucles qui finissent sur une condition

Toute une classe de méthodes numériques se définit par un *jusqu'à* :

- la recherche de racine et l'itération de point fixe à tolérance donnée : Newton, Halley, la sécante ; l'équation implicite d'un étage d'analogique virtuel (un écrêteur à diodes, une échelle à retour sans délai, une saturation en boucle), la résolution non linéaire par échantillon d'un modèle de circuit en ondes ou nodal ;
- le raffinement itératif d'un petit système linéaire par échantillon (Jacobi, Gauss-Seidel, gradient conjugué sur une matrice nodale), arrêté sur le résidu ;
- le contrôle de pas adaptatif : un intégrateur qui retente un pas avec une taille plus petite jusqu'à ce que son estimation d'erreur passe sous la tolérance, le nombre de sous-pas n'étant connu que des tentatives ;
- la recherche linéaire avec repli d'un optimiseur : diviser le pas par deux jusqu'à ce que la perte baisse ;
- une recherche qui s'arrête au premier candidat acceptable : une grille, une bissection sur un paramètre, un redémarrage d'un point perturbé ;
- les deux évaluations d'un pas stochastique (SPSA), ou un redémarrage, dans un seul échantillon plutôt que sur deux trames.

Leur forme commune : un corps avec état, une borne dont le temps réel a de toute façon besoin, et une sortie lue sur l'itéré. La borne est le budget ; la sortie est ce qui fait du budget un maximum et non un coût.

### 1.3 Ce que coûte les écrire sans `repeat`

Elles s'écrivent dès aujourd'hui, et le solveur de Newton du §6.1 l'a été, en deux couches : un `ondemand` entier extérieur dont l'horloge est le budget, décidé par le résidu du démarrage à chaud, et à l'intérieur un `ondemand` booléen qui conditionne chaque pas au résidu de l'itéré maintenu. Cela tourne, c'est exact, et c'est de cela que `repeat` est fait. Ce que cela coûte :

- les itérations inutilisées du budget évaluent chacune un résidu, un `or` et un `if` : 4 à 12 % du solveur sur une sinusoïde à 100 Hz, mesuré ;
- la boucle est étalée sur deux blocs, un retour et un drapeau maintenu. Un lecteur voit deux gardes, pas une boucle, et une bibliothèque ne peut pas documenter « ce bloc s'arrête quand il a convergé » comme le contrat d'une seule construction ;
- la forme doit être reconnue, par les gens et par les outils, chaque fois qu'elle est écrite, et un petit changement (le drapeau rebouclé par la mauvaise voie, une tangente sur le drapeau) en fait silencieusement autre chose.

### 1.4 Pourquoi un nom dans le langage

`repeat` nomme cette forme : la borne comme horloge, la sortie comme dernière sortie du corps, la première itération inconditionnelle, et le modèle de temps des trois primitives existantes. Au-delà de la forme de bibliothèque, qui donne déjà la sémantique, le nom apporte quatre choses, aucune ne concernant le code généré :

- **un contrat.** « Ce bloc tourne au plus `H` fois et s'arrête quand son drapeau tombe » se dit en une construction, lisible par une personne et connue du compilateur ;
- **la borne et la sortie comme règles.** Le coût dans le pire cas d'un échantillon reste `H × corps`, pour les quatre formes de la même façon ; le point d'évaluation du drapeau, après les sorties de son itération, est une règle du langage et non une convention sur un retour ;
- **une règle unique pour `fad`.** Le drapeau ne porte que sa valeur primale ; une tangente ne peut pas atterrir là où la sortie est lue (§7) ;
- **une boucle, trois spécialisations.** Les trois primitives existantes en deviennent les formes dérivées (§4), ce qui est le gain conceptuel de cette note : les deux lectures d'`ondemand` disparaissent, `downsampling` est une expression d'horloge, `upsampling` une réécriture d'entrée, et l'annotation de cadence est la seule chose qui reste et qui n'est pas la boucle.

## 2. `repeat` en une page

```faust
repeat(C)
```

**Arité.** Si `C : u → v+1` alors `repeat(C) : u+1 → v`. Comme pour les trois autres primitives, l'entrée supplémentaire, **la première**, est l'horloge `H`. La **dernière sortie** de `C` est un *drapeau de continuation*, consommé par la primitive et non exposé. `C` doit avoir au moins une sortie en plus du drapeau.

**Par échantillon extérieur**, avec `n = int(H)` (l'horloge tronquée en entier, comme le font les trois primitives ; une horloge négative compte pour 0) :

```tsv
horloge	effet
n = 0	le corps ne tourne pas ; les sorties gardent leur dernière valeur (0 au départ)
n = 1	le corps tourne une fois ; son drapeau est lu mais ne peut rien changer
n ≥ 2	le corps tourne, puis encore tant que son drapeau était non nul, au plus n fois
```

L'horloge est une **borne**, jamais une promesse : le corps tourne entre 1 et `n` fois quand `n > 0`, et la première itération est inconditionnelle. La sortie de boucle est décidée **à l'intérieur**, sur ce que l'itération vient de calculer. En notation à la C, par échantillon extérieur :

```c
if (n > 0) do { body; } while (flag && ++k < n);
```

Un `while` sans borne n'est pas proposé, à dessein : en temps réel, un bloc dont l'itération ne converge pas est un fil audio qui ne revient jamais. `repeat` garde le coût d'un échantillon dans le pire cas borné par l'intervalle de l'horloge, que le système de types connaît.

## 3. La sémantique, précisément

### 3.1 Deux temps

Le programme hors du bloc avance en *temps extérieur* `t = 0, 1, 2, …`, un pas par échantillon audio. Le corps `C` avance en *temps local* `τ = 0, 1, 2, …`, un pas par **itération exécutée**, à travers tous les échantillons extérieurs. Toute construction à état dans le corps (un retard, une récursion `~`, une table, `ba.time`, un oscillateur) avance une fois par itération exécutée et jamais autrement. Une définition référencée dans le corps est instanciée dans le domaine du corps, comme pour les trois primitives ([ondemand-note-fr.md](ondemand-note-fr.md), section 3) : le même `x @ 1` écrit à l'intérieur et à l'extérieur du bloc, ce sont deux lignes de retard.

### 3.2 Les équations

Soient `H` l'horloge et `x = (x₁ … x_u)` les entrées, tous signaux extérieurs. Soit `C` le corps vu comme processeur de signaux en temps local : d'un flux d'entrée `X(τ)` il produit `v` flux de sortie `Y(τ)` et un flux de drapeau `F(τ)`, tout son état avançant en `τ`.

Par échantillon extérieur `t` :

- `n_t = max(0, int(H(t)))`, la borne ;
- `τ_t` est le nombre d'itérations exécutées avant l'échantillon `t` (`τ_0 = 0`) ;
- les entrées sont **figées** : `X(τ) = x(t)` pour toute itération `τ` exécutée à l'échantillon `t`, constantes sur les itérations de cet échantillon ;
- le nombre d'itérations exécutées en `t` est `k_t = 0` si `n_t = 0`, sinon `k_t = min(n_t, 1 + min{ j ≥ 0 : F(τ_t + j) = 0 })` (la première itération dont le drapeau vaut 0 est la dernière exécutée ; si aucun drapeau ne tombe, les `n_t` tournent) ;
- `τ_{t+1} = τ_t + k_t` ;
- les sorties du bloc sont celles de la **dernière itération exécutée**, maintenues sinon : `y(t) = Y(τ_{t+1} − 1)` si `k_t > 0`, sinon `y(t) = y(t−1)`, avec `y(−1) = 0`.

C'est bien fondé : `F(τ_t + j)` dépend de l'état de `C` en `τ_t` et de la valeur figée `x(t)`, tous deux connus avant que l'itération ne tourne.

### 3.3 Le drapeau

Le drapeau est évalué **après** les sorties de son itération. L'itération qui dit « stop » est donc incluse, et les sorties maintenues sont celles qu'elle a produites. Sa polarité est *continuer* : non nul veut dire « on continue », ce qui se lit comme `while (flag)`. (L'alternative, un drapeau d'*arrêt*, que l'idiome `repeat … until` suggérerait, est la seule décision encore ouverte ; rien d'autre dans cette note n'en dépend.)

Le drapeau n'est lu que lorsqu'il y a quelque chose vers quoi continuer : avec `n_t = 1` il est calculé et ignoré, et le système de types peut ne pas le calculer du tout quand l'intervalle de l'horloge est contenu dans [0, 1].

### 3.4 L'horloge

- **Troncature.** Une horloge réelle est convertie en entier avant toute chose, comme pour les trois primitives et comme le fait la référence C++ : `0.5` ne fait jamais tourner le corps, `2.5` le fait tourner au plus deux fois. Il n'y a pas d'exécution fractionnaire.
- **Horloges constantes.** `H ≡ 0` : le corps n'est jamais instancié et les sorties valent 0. `H ≡ 1` : le corps est `C` exécuté une fois par échantillon, sa sortie de drapeau supprimée ; le temps local est le temps extérieur.
- **Intervalle booléen.** Quand le système de types sait que `H ∈ [0, 1]`, au plus une itération tourne par échantillon et le drapeau ne peut rien changer : `repeat` et `ondemand` coïncident.
- **`ma.SR` à l'intérieur est inchangé.** Le corps voit la fréquence d'échantillonnage extérieure, comme sous `ondemand` : le nombre d'itérations par échantillon est un signal, il n'y a pas de rapport constant à replier dans une cadence (§4.4).

### 3.5 Imbrication

Toute primitive à horloge peut apparaître dans toute autre. L'horloge d'un bloc intérieur est un signal du domaine du corps englobant, évaluée une fois par itération englobante. Une sortie de boucle ne quitte que sa propre boucle.

## 4. Les trois primitives comme formes dérivées

La construction a une boucle, un modèle de temps, une règle de maintien et une règle d'entrée. Chacune des trois primitives est `repeat` avec un drapeau constant, plus au plus un de trois ajouts orthogonaux : une expression d'horloge, une réécriture d'entrée, ou une annotation de cadence.

### 4.1 `ondemand`

```faust
ondemand(C) ≡ repeat((C, 1))
```

`(C, 1)` est `C` auquel on adjoint en parallèle un drapeau constant à 1. Le corps tourne `int(H)` fois par échantillon, quel que soit l'intervalle de l'horloge. Les deux lignes de la table d'`ondemand` dans [ondemand-note-fr.md](ondemand-note-fr.md) section 2 (une *condition* quand l'intervalle est contenu dans [0, 1], un *compte* sinon) sont donc **une seule règle**, `int(H) ∈ {0, 1}` étant le cas où un compte est un `if`. La lecture en condition est une émission du compte, pas une seconde sémantique. `ondemand` n'a pas de sortie de boucle : la borne est le compte.

### 4.2 `downsampling`

```faust
// l'horloge divisée : un compteur dans le domaine extérieur, tire quand il vaut 0
fire(H) = (c == 0) with { c = ((+(1) ~ _) - 1) % H; };
downsampling(C) ≡ (fire(H), si.bus(u)) : ondemand(C)      // plus ma.SR = SR / H à l'intérieur
```

Le corps tourne aux instants `t = 0, H, 2H, …`, une fois chacun (le drapeau vaut 1 et l'horloge est booléenne), ses entrées échantillonnées à ces instants, ses sorties maintenues entre eux. Le compteur vit dans le domaine extérieur ; à période constante c'est `t mod H`, mesuré identique échantillon pour échantillon à `downsampling(3)`. Quand la période change entre deux tirs, c'est le compteur qui définit le motif de tir, et c'est une propriété de la forme dérivée à fixer, pas de `repeat`. Ce que `downsampling` ajoute et qu'aucune combinaison de la boucle n'exprime, c'est l'**annotation de cadence** : `ma.SR` dans le corps vaut `SR / H` (§4.4).

### 4.3 `upsampling`

```faust
// bourrage de zéros : l'échantillon arrive à la dernière des H itérations, des zéros avant
stuff(H, i, x) = x * (i == H - 1);                        // i : l'indice d'itération dans l'échantillon
upsampling(C) ≡ (H, x) : repeat((stuff(H, i) : C, 1))    // plus ma.SR = SR * H à l'intérieur
```

Le corps tourne `int(H)` fois par échantillon, le drapeau vaut 1, et les entrées ne sont **pas** la valeur figée répétée mais la valeur figée **bourrée de zéros** : l'entrée `j` de l'itération `i` vaut `x_j(t)` quand `i = H − 1` et 0 avant. Les sorties sont celles de la dernière itération, comme pour toute forme. `upsampling(C)` est donc la chaîne des manuels : bourrage de zéros (`↑₀`, celui qu'`interleave.lib` utilise aussi), `C` à la cadence `SR · H`, et décimation en gardant le dernier de chaque groupe de `H`.

Pourquoi l'échantillon arrive à la **dernière** itération et non à la première : c'est ce qui fait `upsampling(_) = _`, échantillon pour échantillon et sans retard. Avec l'échantillon en premier, la sortie gardée serait celle de la dernière itération, qui a vu un 0. Le même placement est la raison pour laquelle `upsampling` n'admet pas de drapeau de sortie : une sortie anticipée quitterait la boucle avant l'arrivée de l'entrée. Mesuré : l'`ondemand` entier bourré de zéros est identique échantillon pour échantillon à `upsampling(3)`.

Deux conséquences pour le programmeur. Un corps sans mémoire (`upsampling(ma.tanh)`) est l'identité sur le corps, le sur-échantillonnage ne faisant rien ; il n'agit qu'à travers l'**état** du corps, le filtre d'interpolation qui étale l'échantillon sur les `H` positions et le filtre anti-repliement avant la décimation, tous deux à écrire par le programmeur (§6.2). Et le bourrage de zéros divise le niveau par `H`, que le gain du filtre d'interpolation doit rétablir (`xi * H` dans l'exemple du §6.2).

### 4.4 Ce que les formes dérivées ajoutent

```tsv
forme	horloge donnée à la boucle	réécriture d'entrée	drapeau	ma.SR dans le corps
repeat(C)	H, la borne	aucune	la dernière sortie de C	SR
ondemand(C)	H, le compte	aucune	1	SR
downsampling(C)	le tir d'un compteur, 0/1	aucune	1	SR / H
upsampling(C)	H, le compte	bourrage de zéros, échantillon en dernier	1	SR · H
```

L'annotation de cadence est la seule chose que la boucle ne peut pas exprimer. Un corps qui lit `ma.SR` ne veut pas dire la même chose sous `upsampling` que sous `repeat` à horloge égale : un filtre dessiné à partir de `ma.SR` dans `upsampling` est réglé pour la cadence sur-échantillonnée, dans `repeat` ou `ondemand` pour la cadence extérieure. C'est une propriété du **domaine**, posée par les deux primitives de cadence parce que leur facteur est l'horloge, et par rien d'autre. (`maths.lib` borne `ma.SR` à 192 kHz, donc au-dessus de `×4` à 48 kHz un filtre de `filters.lib` dans `upsampling` est dessiné pour la mauvaise cadence, sauf à demander `fc * ma.SR / SRraw`, un dessin qui ne dépend que de `fc / SR` ; voir §6.2.)

### 4.5 Identités

La construction est tenue par ces égalités, échantillon pour échantillon ; les quatre premières ont été mesurées le 2026-09-18, les autres découlent des §3 et §4 :

- `ondemand(C) = repeat((C, 1))` : un drapeau toujours à 1 avec `H = 5` compte 5 itérations par échantillon ;
- `repeat((C, 0))` tourne exactement une fois par échantillon quand `int(H) > 0`, y compris quand `H` change d'un échantillon au suivant : c'est `ondemand(C)` avec l'horloge booléenne `int(H) > 0` ;
- `downsampling(3)` = `ondemand` avec l'horloge divisée du §4.2 ;
- `upsampling(3)` = `ondemand` entier bourré de zéros, §4.3 ;
- `H ≡ 1` : `repeat(C)` est `C` privé de son drapeau, `ondemand(C)`, `upsampling(C)` et `downsampling(C)` sont `C` ;
- `H ≡ 0` : toute forme sort 0 ;
- `upsampling(_) = _`, et `upsampling(C) = C` pour un `C` sans mémoire ;
- `downsampling(_)` est un échantillonneur-bloqueur de période `H` ;
- `il.interleave(N, _) = @(N − 1)` (`ondemand` booléen, `interleave.lib`) ;
- l'imbrication multiplie : `upsampling` de facteur `a` autour d'`upsampling` de facteur `b` fait `a · b` itérations à `SR · a · b` ; `downsampling` de période `a` autour de `downsampling` de période `b` tire tous les `a · b` échantillons à `SR / (a · b)`.

## 5. Ce que vaut la construction

**Une seule sémantique.** Un temps local qui compte les itérations exécutées, des entrées figées à l'échantillon, les sorties de la dernière itération maintenues, une horloge tronquée en borne : toute primitive obéit aux mêmes quatre règles, et ce qui les distingue tient dans la table du §4.4. Rien dans les trois primitives n'exige de règle propre, et la seule dualité apparente, les deux lectures d'`ondemand`, disparaît.

**La sortie appartient à la boucle.** Avant `repeat`, un critère d'arrêt calculé dans l'itération demandait deux couches : un `ondemand` extérieur qui dépense un budget, un `ondemand` booléen intérieur qui conditionne chaque pas au drapeau maintenu du précédent. `repeat` dit la même chose en une couche, la sortie lue sur l'itéré lui-même, et la forme à deux couches devient ce dont `repeat` est fait. Mesuré sur le solveur du §6.1 : les mêmes pas par échantillon que les deux couches et 4 à 12 % de moins, les itérations sautées ne recalculant plus leur résidu.

**La borne reste.** `repeat` n'est pas un `while`. Ce qu'il abandonne est exactement ce que le temps réel ne peut pas avoir ; ce qu'il garde, c'est que l'intervalle de l'horloge borne statiquement le coût d'un échantillon, pour les quatre formes de la même façon, et qu'un budget trop petit est une réponse fausse et non un blocage. Ce dernier point est un avertissement : une itération coupée par la borne est silencieuse. Un solveur écrit avec `repeat` devrait exposer son nombre de pas, ou un drapeau « borne atteinte », pour qu'un programme puisse le voir.

**Les trois primitives sont tenues par la référence C++.** Leur sémantique n'est pas seulement les équations des §3 et §4 mais 39 programmes des tests d'impulsion (21 `ondemand`, 9 `upsampling`, 9 `downsampling`) comparés échantillon pour échantillon à la branche C++ de Faust qui les définit, sur chaque backend. `repeat` n'a pas de contrepartie C++ ; sa sémantique est fixée par la forme de bibliothèque et par les identités du §4.5.

**Là où ce n'est pas uniforme**, dit pour que personne ne soit surpris :

- `ma.SR` (§4.4) : le seul endroit où une primitive est plus qu'une boucle avec une horloge, et celui qui change ce qu'un corps veut dire.
- La troncature d'une horloge réelle (§3.4) : `0.5` est une horloge qui ne tire jamais. La première version de la note `ondemand` disait le contraire et a été corrigée le 2026-09-18.
- `upsampling` n'admet pas de sortie et `downsampling` n'en a pas à admettre (§4.3) : le drapeau n'a de sens que pour `repeat`, et pour `ondemand` seulement comme constante.
- La première itération est inconditionnelle (§2). Pour sauter un échantillon entier, la décision se prend à l'extérieur sur l'état maintenu, par une horloge à 0, ce qui est la façon d'écrire « ne rien faire tant que l'entrée n'a pas bougé » (§6.1).
- La polarité du drapeau (§3.3) n'est pas décidée.

## 6. Usages emblématiques

### 6.1 Un solveur implicite à nombre de pas décidé à l'exécution

`newton(N, F, y0)` d'`optimizers.lib` déroule `N` pas dans le graphe et repart de `y0` à chaque échantillon. Avec `repeat`, l'itération est une récursion dans le corps, l'état du bloc est l'itéré (l'échantillon suivant part de la solution précédente, un démarrage à chaud gratuit), le nombre de pas est décidé par le résidu de l'itéré, et le code a la taille d'un pas. Sur la saturation en boucle `y = tanh(x − fb · y)` :

```faust
fb = hslider("fb", 0.8, 0, 0.99, 0.01);                 // le retour, le paramètre contre lequel le gradient du §7 est pris
tol = 1e-12;                                             // tolérance sur le résidu
Kmax = 8;                                                // le budget : au plus 8 pas par échantillon
F(x, y) = y - ma.tanh(x - fb * y);                       // résidu
step(x, y) = y - (fad(F(x, y), y) : /);                  // un pas de Newton, y - F / F_y
iter(xi) = (step(xi) ~ _) <: _, (abs(F(xi, _)) > tol);   // (nouvel itéré, continuer tant que non convergé)
solve(x) = sel ~ _
with { sel(yp) = ((abs(F(x, yp)) > tol) * Kmax, x) : repeat(iter); };
```

À l'extérieur, le résidu du démarrage à chaud sur la nouvelle entrée décide l'horloge : 0 quand la solution maintenue satisfait déjà la tolérance (le bloc ne tourne pas, la sortie est maintenue), le budget `Kmax` sinon. À l'intérieur, un pas par itération tant que le résidu du nouvel itéré est au-dessus de la tolérance. Avec `tol` à 1e-12 et `Kmax` à 8, contre `newton(8, …)` depuis 0 (10,2 ms par seconde d'audio), égal à lui à 1,1e-12 près :

```tsv
entrée	pas par échantillon, moyenne et maximum	coût par seconde d'audio
constante	0 après le premier échantillon	0,25 ms
sinusoïde, 100 Hz	2,4, 3	5,3 ms
sinusoïde, 2 kHz	3,0, 3	6,1 ms
bruit blanc	3,5, 5	7,3 ms
```

Un solveur qui ne coûte rien tant que son entrée ne bouge pas, et les pas qu'il lui faut sinon. La même forme est un écrêteur à diodes, une échelle à retour sans délai, tout étage d'analogique virtuel à non-linéarité implicite, et toute itération de point fixe dont la convergence ne se lit que sur son itéré.

### 6.2 Une non-linéarité sur-échantillonnée, en `upsampling`

Un waveshaper appris sous une perte spectrale a besoin du sur-échantillonnage, le repliement étant une erreur spectrale que le gradient apprendrait sinon à annuler. `upsampling` est l'étage sur-échantillonné en un bloc : le bourrage de zéros par contrat, un filtre d'interpolation à la cadence intérieure, la non-linéarité, un filtre anti-repliement, la dernière itération comme décimation :

```faust
H = hslider("factor", 4, 1, 16, 1);                     // le facteur de sur-échantillonnage, un signal
SRraw = fconstant(int fSamplingFreq, <math.h>);
lp = fi.lowpass(6, 20000 * ma.SR / SRraw);            // dessiné à la cadence intérieure, voir §4.4
stage(x) = (H, x) : upsampling(\(xi).(xi * H : lp : ma.tanh : lp));
```

Mesuré sur une sinusoïde à 5250 Hz à un gain d'attaque de 8, comme rapport de l'énergie harmonique au reste du spectre :

```tsv
facteur	harmonique / reste	coût par seconde d'audio
ma.tanh nu	13,5 dB	
2	30,7 dB	2,0 ms
4	31,4 dB	3,8 ms
8	30,1 dB	7,3 ms
```

Le plateau est la bande de transition du Butterworth d'ordre six, pas celle du mécanisme. Le facteur peut être un signal (`×4` tant qu'un suiveur d'enveloppe est au-dessus d'un seuil, `×1` en dessous) ; le filtre intérieur devient alors variable dans le temps au moment du basculement. Le gradient traverse le bloc : la dérivée de la sortie quadratique moyenne par rapport au gain d'attaque coïncide avec la différence centrée à 2,9e-6 près à `×4`.

### 6.3 Plusieurs pas d'optimiseur par tir, en `ondemand`

Avec `H = K · frame_clock(N)`, une boucle de descente entière tourne `K` fois au tir de trame et jamais entre deux, repartant à chaud de la trame précédente : l'analyse par la synthèse, trame par trame, du DDSP.

```faust
K = 4;                                                   // pas d'optimiseur par tir de trame
learn(x) = (K * il.frame_clock(64), x)
         : ondemand(\(xi).(op.descend_1D(\(p).(op.mse(p * xi, 0.7 * xi)), op.sgd_g(0.1), -4, 4, 0, 0)));
```

Sur un gain parti de 0 avec SGD à 0,1 : avec `K = 1` le paramètre est à 0,692 après 20 trames ; avec `K = 4` à 0,7 à 1e-8 près après 20 ; avec `K = 16` après 5. Le prix est l'endroit où tombe le travail, sur un seul échantillon ; la forme booléenne l'étale sur la trame au prix d'une trame de latence.

### 6.4 Une recherche comme boucle, avec sortie

Une grille de `M` candidats essayés un par itération, le candidat fonction du temps local, l'état gardant le meilleur : un code de la taille d'une évaluation là où `multistart_1D` copie le graphe `M` fois.

```faust
M = 16;                                                  // candidats
clock = il.frame_clock(64);                              // une grille par tir de trame
loss(p, x) = op.mse(p * x, 0.37 * x);                    // le gain cible est 0,37
body(xi) = best ~ (_, _)
with {
    i = ((+(1)) ~ _) - 1 : %(M);                  // temps local : l'indice d'itération
    cand = -1.0 + 2.0 * i / (M - 1);
    best(pb, lb) = select2(better, pb, cand), select2(better, lb, l)
    with { l = loss(cand, xi); better = (l < lb) | (i == 0); };
};
grid = (M * clock, _) : ondemand(body);
```

Seize candidats sur [−1, 1] pour une cible de 0,37 élisent 0,333 en un tir. Ce que `repeat` ajoute, c'est la sortie. La même grille, arrêtée au premier candidat dont la perte passe sous un seuil, l'horloge la bornant toujours à `M` :

```faust
thr = 1e-3;                                              // perte acceptable
body(xi) = (best ~ (_, _)) : (_, >(thr))                 // (meilleur candidat, continuer tant que sa perte est au-dessus du seuil)
with {
    i = ((+(1)) ~ _) - 1 : %(M);
    cand = -1.0 + 2.0 * i / (M - 1);
    best(pb, lb) = select2(better, pb, cand), select2(better, lb, l)
    with { l = loss(cand, xi); better = (l < lb) | (i == 0); };
};
grid = (M * clock, _) : repeat(body);
```

Sur une entrée constante le bloc élit le même 0,333 et s'arrête dès qu'un candidat passe sous le seuil, là où la forme `ondemand` parcourt toujours les `M` candidats. Plus généralement, la sortie : un drapeau « pas encore de candidat sous le seuil » arrête la grille au premier acceptable ; un pas divisé par deux à chaque itération avec le drapeau « la perte n'a pas encore baissé » est une recherche linéaire avec repli, qui s'arrête à l'exécution là où les formes d'aujourd'hui dépensent tout leur budget ; les deux évaluations d'un pas SPSA tiennent dans un échantillon au lieu de deux trames.

### 6.5 Là où la sortie n'est connue qu'à l'intérieur

Un intégrateur adaptatif (RK45 avec rejet de pas sur un circuit raide) ne connaît son nombre de sous-pas que par l'estimation d'erreur de chaque tentative : un `repeat` dont le drapeau est « le pas a été rejeté », avec la borne comme garde-fou. À l'inverse, un rééchantillonneur rationnel reste un compte : le nombre d'échantillons de sortie par échantillon d'entrée se lit sur l'accumulateur de phase avant la boucle, c'est donc un `ondemand` entier, `repeat` avec un drapeau à 1.

### 6.6 Les formes booléennes sont inchangées

Le travail déclenché par événement (`ondemand` à horloge 0/1), le calcul à cadence de contrôle (`downsampling` avec une période), le traitement spectral à cadence de trame (`interleave.lib` sur un `ondemand` booléen) : tout ce que décrivent les sections 4 et 5 de [ondemand-note-fr.md](ondemand-note-fr.md) est `repeat` avec un drapeau constant et une horloge booléenne, et se lit exactement comme avant.

## 7. Différentiation

`fad` traverse `repeat` comme il traverse les trois primitives : chaque sortie maintenue porte sa valeur primale et ses tangentes, le drapeau ne porte que sa valeur primale. L'horloge et le drapeau sont opaques à la dérivée, ce qui est juste : un nombre d'itérations et une comparaison sont discrets. Quand le corps est une itération contractante qui sort à la convergence, la tangente à la sortie est la dérivée du point fixe (Christianson, 1994), et le bloc donne la dérivée implicite sans règle propre. Mesuré sur le solveur du §6.1 : la tangente de la solution par rapport à `fb` sur une entrée constante vaut −0,240418, la différence centrée −0,24042. `rad` ne traverse pas une frontière d'horloge, `repeat` compris ; l'apprentissage à travers ces blocs passe par `fad`.

## 8. Points ouverts, de sémantique seulement

Quatre questions restent à décider. Aucune ne touche au modèle du §3 ; chacune fixe une convention dont tout programme écrit avec `repeat` dépendra, ce qui est la raison de les lister ici plutôt que de les laisser à l'implémentation.

- **La polarité du drapeau** (§3.3). La dernière sortie du corps peut vouloir dire « continuer » (non nul : on recommence) ou « arrêter » (non nul : on sort). Le modèle est le même dans les deux cas ; ce qui change, c'est le sens d'un bit, et il change dans chaque programme : un corps dont le drapeau est `abs(F) > tol` dans la lecture « continuer » doit devenir `abs(F) <= tol` dans la lecture « arrêter », et un programme écrit pour l'une et compilé sous l'autre fait une itération là où il devrait aller jusqu'à la convergence, ou dépense tout le budget là où il devrait s'arrêter tout de suite. Pour *continuer* : c'est ce que la forme de bibliothèque du §1.5 du plan implémente et mesure, et cela suit la convention d'`op.gated`, dont la dernière sortie est une porte où 1 veut dire « actif ». Pour *arrêter* : le nom se lit `repeat … until`, et un test de convergence est naturellement une condition d'arrêt (« convergé, donc stop »). La décision doit être prise avant le premier programme, et ne pourra plus être revue ensuite sans changer leur sens.

- **Le corps minimal.** Le corps peut-il avoir le drapeau pour seule sortie ? Une boucle avec état et sans sortie n'a pas d'effet observable en Faust, qui n'a pas d'effets de bord ; l'accepter, ce serait admettre un bloc qui ne calcule rien que quiconque puisse lire. La proposition est de la refuser, comme on refuse un bloc `gated` sans sortie en plus de sa porte. L'alternative, l'accepter comme un bloc à `u+1` entrées et aucune sortie, ne coûte rien au modèle et n'apporte rien non plus ; l'enjeu est seulement de dire laquelle, pour que la règle d'arité `C : u → v+1`, `repeat(C) : u+1 → v` porte ou non `v ≥ 1`.

- **Une période qui change sous `downsampling`** (§4.2). La forme dérivée compte dans le domaine extérieur, et deux compteurs coïncident tant que la période est constante et divergent quand elle change. Avec `t mod H(t)`, une période de 3 pour `t = 0 … 4` puis de 2 tire en `t = 0, 3, 6`. Avec un compteur qui reboucle, `c ← (c + 1) mod H` évalué à chaque instant et tirant quand `c = 0`, la même horloge tire en `t = 0, 3, 5` : le compteur vaut 1 en `t = 4` et reboucle sur 2 à l'instant suivant. La mesure du 2026-09-18 ne couvrait que la période constante. Le compteur est ce que l'émission réalise et ce que fait la référence C++, c'est donc le comportement à écrire comme règle ; l'enjeu est qu'il soit écrit, puisqu'un programme dont la période vient d'un slider le rencontrera.

- **Les horloges négatives.** `int(H)` peut être négatif quand l'horloge vient d'un slider ou d'une soustraction. La règle proposée est qu'une borne négative compte pour 0 : le corps ne tourne pas et les sorties sont maintenues, exactement comme pour `H = 0`. Les alternatives, une erreur quand l'intervalle de l'horloge admet des valeurs négatives, ou la valeur absolue, se défendent toutes deux ; la première refuserait des programmes qui tournent aujourd'hui, la seconde ferait tourner un corps que le programmeur n'a pas demandé. Quel que soit le choix, il doit être une règle et non ce que l'en-tête de boucle fait par hasard d'une borne négative. Ce qu'il fait aujourd'hui, vérifié le 2026-09-19 sur une horloge venant d'un slider d'intervalle [−4, 4] : la référence C++ (`8eebea429`, 2.84.3) émet `if (H != 0) { for (od = 0; od < H; od++) … }` et faust-rs `for (lOd = 0; lOd < H; lOd++)`, donc sur les deux une horloge négative passe la garde, fait zéro tour de boucle et maintient les sorties, exactement comme `H = 0` ; aucun des deux compilateurs ne refuse l'intervalle ni n'avertit. La règle proposée est donc le comportement actuel, rendu explicite.

## Voir aussi

- [ondemand-note-fr.md](ondemand-note-fr.md) — les trois primitives, côté programmeur
- [ondemand-fft-spectral-comparison-en.md](ondemand-fft-spectral-comparison-en.md) — le traitement à cadence de trame sur un `ondemand` booléen
- [fad-note-en.md](fad-note-en.md) — différentiation en mode direct
- `libraries/optimizers-overview-fr.md` section 2.5 — horloges entières et `upsampling` pour le DDSP, avec les programmes et les mesures cités ici
- `porting/repeat-counted-loop-with-exit-analysis-and-plan-2026-09-18-en.md` — l'analyse, la forme de bibliothèque, la conception côté compilateur et son plan
