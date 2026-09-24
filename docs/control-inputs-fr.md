# Les entrées de contrôle comme boîtes : `cinputs`, `cinput`, `coutputs`, `coutput` et la cible de modulation joker `"*"`

Version anglaise : [control-inputs-en.md](control-inputs-en.md) (même
contenu ; garder les deux versions synchronisées).

Extensions de faust-rs, inconnues du compilateur Faust C++, de la même famille
que `fad` et `rad`. Contrat et justification :
`porting/control-inputs-and-wildcard-modulation-analysis-2026-09-22-en.md`.
Implémentation : `crates/eval/src/control_inputs.rs`, le joker dans
`crates/eval/src/modulation.rs`, la liste elle-même dans
`crates/propagate/src/control_widgets.rs`. Tests :
`crates/compiler/tests/control_inputs.rs`, sur les fixtures
`tests/corpus/cinputs_*.dsp`, `tests/corpus/wildcard_*.dsp` et
`tests/corpus/err_3{0,1}_*.dsp`.

## 1. Les quatre primitives

```faust
cinputs(e)       // les entrées de contrôle de e, liste de ses boîtes widgets
cinput(i, e)     // la i-ème entrée de contrôle, à partir de 0, soit (widget, init, min, max, step)
coutputs(e)      // les bargraphs de e, liste de ses boîtes bargraphs
coutput(i, e)    // le i-ème bargraph, à partir de 0, soit (bargraph, min, max)
```

- **Ce qui compte.** Une entrée de contrôle est un `hslider`, `vslider`,
  `nentry`, `button` ou `checkbox` ; un bargraph est un `hbargraph` ou un
  `vbargraph`. Un soundfile n'est ni l'un ni l'autre.
- **Ce qui est listé.** Les entrées de contrôle présentes dans `e`, lues ou
  non, comme `inputs(e)` compte les entrées audio de `e` : `inputs(_ : !)` vaut
  1 bien que l'entrée soit coupée, et `outputs(cinputs(hslider("dead",…) : !))`
  vaut 1 bien que le curseur le soit. La liste est prise sur la boîte, à
  l'évaluation, avant que les signaux n'existent ; elle ne dépend pas de ce que
  le compilateur éliminera ensuite en simplifiant. C'est donc un sur-ensemble
  de l'interface compilée, qui ne montre que les widgets encore lus par les
  signaux finaux : `hslider("d",…) * 0 + hslider("l",…)` liste `d` et `l`, son
  interface montre `l`.
- **Ordre.** L'ordre de l'interface que `e` montrerait si tous ses widgets
  étaient lus, celui de son `buildUserInterface` et de son JSON : groupes de
  même label fusionnés, et dans chaque groupe les enfants, contrôles et
  groupes mêlés, triés par leur label brut, préfixe d'ordre `[n]` compris,
  comme le compilateur C++ les trie. Les widgets que l'interface montre sont
  dans son ordre ; un widget mort garde sa place parmi eux. L'ordre de
  déclaration ne compte pas :
  `hslider("b",…) + hslider("a",…) + hgroup("z", hslider("c",…)) + hslider("[0]y",…)`
  liste `y`, `a`, `b`, `z/c`.
- **Identité.** Un widget atteint par plusieurs chemins du programme est un
  seul contrôle ; la même boîte widget sous deux groupes différents
  (`par(i, 3, vgroup("Op %i", g))`) est un contrôle par groupe, comme dans
  l'interface.
- **Le nombre** est `outputs(cinputs(e))`, une constante de compilation
  utilisable comme nombre d'itérations. Un programme sans entrée de contrôle
  donne la boîte vide `0 : !`, donc un nombre nul.
- **Ce qu'est `cinputs(e)`.** Le `par` des boîtes widgets, les mêmes nœuds que
  dans `e` : un bus dont le i-ème signal est `ba.take(i + 1, cinputs(e))`, et
  une liste de graines que `fad` et `rad` prennent telle quelle.
  `fad(f, cinputs(f))` égale `fad(f, (a, b))` écrit avec les widgets. Une
  réserve : une boîte widget répétée sous plusieurs groupes donne plusieurs
  entrées, mais la règle des graines de `fad` et `rad` ramène toute référence
  à un widget pris comme graine à un seul contrôle, si bien que l'ensemencer
  fusionne ses copies.
- **Ce qu'est `cinput(i, e)`.** Cinq boîtes : le widget, le même nœud que dans
  `e`, puis sa valeur par défaut, son minimum, son maximum et son pas évalués
  (pour un bouton ou une case à cocher : `0, 0, 1, 1`). On sélectionne avec un
  motif de coupure : `cinput(i, e) : (!, _, !, !, !)` est la valeur par
  défaut.
- **Ce que sont `coutputs(e)` et `coutput(i, e)`.** La même chose pour les
  bargraphs ; une boîte bargraph a une entrée et une sortie, donc
  `coutputs(e)` est un bus de N entrées et N sorties, et lire un bargraph
  revient à lui fournir son signal.
- **Évaluation.** À l'évaluation des boîtes, comme `inputs(e)` : `e` est
  évalué et abaissé, puis replié. `e` doit être un diagramme fermé ; une
  fonction de signaux (`e(x) = …`) en est un.
- **Erreurs.** Un indice qui n'est pas un entier connu à la compilation,
  négatif, ou égal ou supérieur au nombre, donne `FRS-EVAL-0009` ; le message
  nomme l'indice tel qu'il est écrit, et quand l'expression est elle-même une
  constante (`cinput(freq, 0)`), il signale que les arguments semblent
  inversés et propose `cinput(0, freq)`. Une expression qui n'est pas un
  diagramme donne `FRS-EVAL-0099`.

## 2. La cible de modulation joker `"*"`

```faust
P, x : ["*": (!, _) -> e]     // chaque entrée de contrôle de e remplacée par une entrée, dans l'ordre de cinputs
["*": *(0.5) -> e]            // chaque entrée de contrôle divisée par deux
["amp/*": (!, _) -> e]        // chaque entrée de contrôle sous le groupe `amp`
```

- **Correspondance.** Une cible dont le dernier segment est `*` correspond à
  toute entrée de contrôle dont le chemin de groupes contient les segments
  précédents dans l'ordre, comme pour une cible littérale (sous-suite, groupe
  le plus intérieur d'abord) ; `"*"` seul correspond à toutes les entrées de
  contrôle. Les bargraphs ne correspondent jamais. `*` est un segment entier :
  `"stage*"` est un label littéral. Un préfixe de groupe s'écrit sans son
  type, `"amp/*"` et non `"h:amp/*"`, comme pour les cibles littérales.
- **Une entrée par contrôle.** Avec un modulateur à deux entrées, le joker
  ajoute une entrée **par contrôle trouvé**, dans l'ordre de `cinputs`, devant
  les entrées de `e` : la i-ème entrée ajoutée pilote le contrôle que décrit
  `ba.take(i + 1, cinputs(e))` ; un contrôle mort reçoit aussi la sienne,
  qu'il ignore, si bien que `"*"` ajoute `outputs(cinputs(e))` entrées. Un
  label littéral qui correspond à plusieurs widgets leur donne une seule
  entrée partagée, comme en C++ ; le joker, non. `["*": (!, _) -> e]` égale la
  même modulation écrite avec une cible littérale par contrôle, dans l'ordre
  de l'interface.
- **Arité du modulateur.** Comme pour une cible littérale : 0 entrée remplace
  chaque contrôle trouvé par le modulateur, 1 entrée transforme chacun, 2
  entrées associent chacun à sa propre entrée ajoutée. Seule la forme à 2
  entrées ajoute des entrées.
- **L'interface.** Un contrôle remplacé par `(!, _)` n'est plus lu et quitte
  l'interface.
- **`fad` et `rad` dans `e`.** Une graine qui est un widget de `e` est le même
  contrôle que son usage dans le corps : les deux sont rebranchés sur la même
  entrée.
- **Aucune correspondance** est une erreur, `FRS-EVAL-0010`, là où une cible
  littérale qui ne correspond à rien est l'avertissement `FRS-EVAL-0008` et
  une entrée pendante, comme en C++. Le compilateur C++ lit une cible joker
  comme un label et ne trouve rien : un programme qui l'utilise y échoue avec
  son comportement « aucune correspondance », pas sur une erreur de syntaxe.

### Un piège des cibles littérales

`["a", "b": m -> e]` n'attache `m` qu'à `b` ; `a` reçoit le modulateur par
défaut `*`. Donnez son modulateur à chaque cible : `["a": m, "b": m -> e]`, ou
utilisez `"*"`.

## 3. À quoi elles servent

Chaque usage ci-dessous s'applique à un programme `e` tel quel,
`component("x.dsp")` ou toute expression fermée, sans le modifier. La plupart
sont regroupés dans `libraries/controls.lib` (préfixe `ct`, section 4), qui
n'importe rien ; les opérateurs d'apprentissage sont dans
`libraries/optimizers.lib`.

### 3.1 Apprendre les contrôles

Un programme qui apprend ses propres curseurs sans être réécrit, dans
`libraries/optimizers.lib` (0.11.0) :

```faust
op = library("optimizers.lib");
e = component("model.dsp");
clock = (ba.time % 2048) == 2047;
process(x, t) = op.adaptive_fad(e, op.mse, op.adam_g(0.01, 0.9, 0.999, 1e-8), clock, button("reset"), x, t);
```

`adaptive_fad` / `adaptive_rad` lisent le nombre de contrôles avec
`outputs(cinputs(e))`, leurs bornes et leurs valeurs par défaut avec `cinput`,
et les rebranchent avec `["*": (!, _) -> e]` ; voir leur documentation dans la
bibliothèque. Piloté par l'hôte, `fad(loss, cinputs(e))` ou
`rad(loss, cinputs(e))` donne le gradient par rapport à chaque contrôle de
`e`.

### 3.2 Le motif derrière les autres : une fonction de chaque contrôle

`cinput(i, e)` donne le widget de chaque contrôle avec sa valeur par défaut,
sa plage et son pas, `["*": (!, _) -> e]` donne à `e` une entrée par contrôle ;
un `par` sur les contrôles fournit donc à `e` n'importe quelle fonction de
ceux-ci :

```faust
N = outputs(cinputs(e));
f(i) = ...;   // construite à partir de cinput(i, e)
process = par(i, N, f(i)) : ["*": (!, _) -> e];
```

Les boîtes widgets que lit `f(i)` sont les nœuds de `e` : une fonction qui lit
son widget garde le bouton dans l'interface, une fonction qui l'ignore le
retire. La valeur par défaut, le minimum, le maximum et le pas sont des
constantes de compilation : ils peuvent régler les paramètres d'un nouveau
widget, dimensionner un `par` ou mettre un pas d'apprentissage à l'échelle.
`ct.map(f, e)` est ce motif, `f(i, w)` recevant l'indice et le widget.

Un piège : pour passer les constantes à un widget, appliquez-lui la fonction,
`w(i, ct.init(i, e), ...)` ; une abstraction composée avec
`cinput(i, e) : \(w, v, a, b, s).(hslider("P", v, a, b, s))` reçoit des
signaux, pas des constantes, et le widget est refusé (`FRS-EVAL-0011`), comme
le compilateur C++ refuse tout paramètre de widget qui n'est pas un nombre.

### 3.3 Usages

| Usage | Comment | `controls.lib` |
|---|---|---|
| Pas de bruit de « fermeture éclair » sur n'importe quel programme | un filtre à un pôle sur chaque contrôle, partant de sa valeur par défaut | `smooth(t, e)`, `smoother(t)` |
| Un module modulaire, chaque bouton avec sa prise | une entrée CV par contrôle, ajoutée au bouton, mise à l'échelle de sa plage, bornée | `cv(depth, e)` |
| Des paramètres pilotés par un hôte ou un autre programme | chaque contrôle devient une entrée audio, dans ses unités ou normalisée dans `[0, 1]` | `external(e)`, `normalized(e)` |
| Une nouvelle interface : knobs, entrées numériques, autres groupes | de nouveaux widgets construits avec la valeur par défaut, la plage et le pas de chaque contrôle | `relabel(wdg, e)`, `knobs(e)` |
| Morphing de presets | des boutons vers une liste de preset, une seule quantité pour tous | `morph(m, P, e)` |
| « Randomize » | un tirage par contrôle sur déclenchement, uniforme sur sa plage et la grille de son pas, mêlé au bouton | `randomize(trig, amount, e)` |
| Voix à l'unisson, écartement stéréo, doublures « humanisées » | des copies de `e` dont chaque contrôle est décalé d'une fraction de sa plage | `offset(d, e)`, dans un `par` |
| Un test sans rien connaître du programme | chaque contrôle balayé sur sa plage ; un compte des sorties non finies | `sweep(T, e)`, `sweep_one(k, T, e)`, `nonfinite(n)` |
| Quels boutons comptent ici | les dérivées des sorties par rapport à chaque contrôle | `gradient_fad(e)`, `gradient_rad(e)` |
| Apprendre les contrôles | une descente sur tous les contrôles vers une cible | `op.adaptive_fad`, `op.adaptive_rad` |

Mesuré sur `ctl_06_testing.dsp` :
`ct.sweep(100, 1 / hslider("d", 0.5, -1, 1, 0.01)) : ct.nonfinite(1)` vaut 1
aux échantillons où le balayage traverse `d = 0` et 0 ailleurs : le genre de
réglage qu'un test écrit à la main oublie.

### 3.4 Limites

- **Les labels ne sont pas des valeurs.** `cinput` ne donne pas de label :
  une interface reconstruite nomme ses widgets par leur indice (`P0`, `P1`,
  ...), et l'interface trie les labels comme du texte, si bien qu'au-delà de
  dix contrôles `P10` vient avant `P2`. Renommer d'après l'ancien label
  demanderait une primitive donnant le i-ème label pour l'interpolation des
  labels.
- **`"*"` touche aussi les boutons et les cases à cocher.** Lisser tous les
  contrôles lisse un `gate` ; restreignez avec une cible de groupe,
  `["synth/*": ct.smoother(t) -> e]`, écrite dans le programme puisqu'une
  cible est un littéral.
- **Les contrôles morts comptent** (section 1) : `map` leur donne une entrée
  qu'ils ignorent.
- **Les bargraphs sont en lecture seule.** `coutputs(e)` donne les boîtes
  bargraphs et leurs plages, pas les signaux que `e` leur envoie : faire des
  mesures d'un programme des sorties, pour les enregistrer ou apprendre
  dessus, demanderait une primitive qui expose ces signaux.
- **Rien ne règle un bouton.** Un programme ne peut pas déplacer ses propres
  curseurs ; `randomize` et `morph` remplacent ce que lit le programme, les
  boutons gardent leur position.

## 4. `controls.lib`

`libraries/controls.lib` (0.1.0, préfixe `ct`) regroupe la section 3 ; elle
n'importe rien, un programme n'a donc besoin que de `-I libraries`. Sections
et fonctions :

- **Lire les contrôles :** `count(e)`, `widget(i, e)`, `init(i, e)`,
  `lo(i, e)`, `hi(i, e)`, `step(i, e)`, `inits(e)`.
- **Rebrancher les contrôles :** `map(f, e)`, `external(e)`,
  `normalized(e)`, `cv(depth, e)`, `smooth(t, e)`, `smoother(t)`.
- **Reconstruire l'interface :** `relabel(wdg, e)`, `knobs(e)`.
- **Explorer les réglages :** `morph(m, P, e)`, `randomize(trig, amount, e)`,
  `offset(d, e)`.
- **Tester :** `sweep(T, e)`, `sweep_one(k, T, e)`, `nonfinite(n)`.
- **Sensibilité :** `gradient_fad(e)` (chaque sortie, puis ses dérivées par
  rapport à chaque contrôle), `gradient_rad(e)` (les sorties, puis le
  gradient de leur somme).

```faust
ct = library("controls.lib");
e = component("synth.dsp");
process = hgroup("synth", ct.knobs(ct.smooth(0.02, e)));
```

Une fonction qui prend `e` demande que `e` ait au moins une entrée de contrôle
(le joker de `map` qui ne trouve rien donne `FRS-EVAL-0010`). Les entrées
qu'ajoute une fonction viennent en premier, une par contrôle dans l'ordre de
`cinputs`, puis les entrées de `e`. Chaque fonction est documentée dans la
bibliothèque avec une entrée `#### Test`, compilée et exécutée par
`tests/corpus/ctl_all_functions.dsp` ; `tests/corpus/ctl_01` à `ctl_07`
donnent leurs sorties attendues, vérifiées par
`crates/compiler/tests/controls_lib.rs`.
