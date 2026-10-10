# Proposition : `nffunction`, fonction externe à sorties multiples

Date : 2026-10-08

Statut : **proposition de langage et de compilation, non implémentée**. Les
exemples utilisant `nffunction` ne compilent pas encore avec `faust-rs` ni avec
le compilateur Faust C++ de référence. Le nom proposé est `nffunction` ;
`ffunction` conserve son contrat actuel.

Version anglaise : [nffunction-multiple-outputs-proposal-2026-10-08-en.md](nffunction-multiple-outputs-proposal-2026-10-08-en.md).
Maintenir les deux versions synchronisées.

## 1. Motivation

`ffunction` décrit une fonction externe à N arguments scalaires et un seul
résultat scalaire. Pour une FFT, un opérateur de trame ou une fonction qui
produit plusieurs valeurs liées, on souhaite plutôt un opérateur **N→M** :
un appel consomme N valeurs et produit M résultats.

La proposition introduit un appel externe commun à toutes les sorties.
Le compilateur prépare les arguments, appelle la fonction, puis expose
ses résultats. Pour une FFT par trame, `ondemand` fournit la cadence d'appel.

Les objectifs sont :

- une signature Faust N→M, avec des types d'entrée et de sortie indépendants ;
- une notation compacte pour les groupes de valeurs de même type ;
- une ABI C à deux pointeurs, même lorsque N et M sont grands ;
- un appel commun aux M sorties, exécuté dans son domaine d'horloge ;
- aucun protocole utilisateur de jetons, d'écritures ou de lectures externes.

## 2. Syntaxe Faust proposée

### 2.1 Types scalaires

```faust
op = nffunction(
    (int, float, float) foo_f32|foo_f64|foo_f80|foo_fixed(float, int, float),
    <foo.h>, ""
);
```

La liste avant le nom décrit les sorties ; la liste après le nom décrit les
entrées. Cet opérateur a trois entrées et trois sorties :

| Position | Entrée | Sortie |
|---|---|---|
| 0 | `float` | `int` |
| 1 | `int` | `float` |
| 2 | `float` | `float` |

Les variantes du symbole, le fichier d'en-tête et l'indication de bibliothèque
reprennent la convention de `ffunction`. Ces noms sont illustratifs : chaque
variante effectivement utilisée doit exister dans la bibliothèque externe.
La première implémentation peut cibler `-single` et `-double` ; les modes de
précision non pris en charge doivent produire un diagnostic explicite.

### 2.2 Répétition d'un type

```faust
fft1024 = nffunction(
    (float[1024]) fft1024f|fft1024d|fft1024l|fft1024fx(float[1024]),
    <fft.h>, ""
);
```

Dans un prototype `nffunction`, `float[K]` signifie **K ports scalaires
successifs de type `float`**. C'est une répétition dans la signature ; elle
n'introduit pas un signal tableau dans le langage Faust.

Les groupes sont concaténés dans l'ordre écrit :

```faust
op = nffunction(
    (int, float[1024]) foo_f32|foo_f64|foo_f80|foo_fixed(float[1024], int),
    <foo.h>, ""
);
```

Cette signature décrit 1025 entrées et 1025 sorties. L'entier d'entrée occupe
le dernier port ; l'entier de sortie occupe le premier.

La taille K doit être une expression entière constante, évaluée à la
compilation, strictement positive et compatible avec les limites du compilateur.
Par exemple, `float[N+2]` est admissible si N est connu à la compilation.
Une taille dynamique, négative, nulle ou un débordement de la somme des arités
doit être rejeté. La première version se limite à `int` et `float`.

### 2.3 Nombre de sorties d'une FFT

Le nom `fft1024f` ne détermine pas le format du spectre. Le prototype doit
correspondre au contrat de la fonction C :

| Transformation et rangement | Entrées scalaires | Sorties scalaires |
|---|---|---|
| FFT réelle, spectre compact dans N réels | N | N |
| FFT réelle, N/2+1 complexes intercalés réel/imaginaire | N | N+2 |
| FFT complexe complète, complexes intercalés | 2N | 2N |

L'exemple `fft1024` ci-dessus suppose un spectre réel compact de 1024 valeurs,
dont le rangement doit être documenté dans `fft.h`. Pour un spectre réel avec
513 complexes intercalés, on écrirait :

```faust
rfft1024 = nffunction(
    (float[1026]) rfft1024f|rfft1024d|rfft1024l|rfft1024fx(float[1024]),
    <fft.h>, ""
);
```

Convention de signe, normalisation et rangement des complexes relèvent du
contrat de la fonction externe. Le compilateur ne les déduit pas du nom.

## 3. Utilisation avec `ondemand`

### 3.1 Entrées déjà disponibles en parallèle

```faust
si = library("signals.lib");
il = library("interleave.lib");

fft1024 = nffunction(
    (float[1024]) fft1024f|fft1024d|fft1024l|fft1024fx(float[1024]),
    <fft.h>, ""
);

// 1024 entrées, 1024 sorties ; un déclenchement tous les 1024 échantillons.
process = (il.frame_clock(1024), si.bus(1024)) : ondemand(fft1024);
```

Pour `F : N→M`, `ondemand(F) : N+1→M` : la première entrée est l'horloge, les
N suivantes sont les données. Ici, l'horloge est booléenne. À chaque
déclenchement :

1. les N valeurs d'entrée sont échantillonnées ;
2. un seul appel externe calcule les M résultats ;
3. les M résultats deviennent les nouvelles sorties du bloc.

Entre deux déclenchements, les sorties restent constantes. Avant le premier
déclenchement, elles valent zéro, avec le type de chaque port. L'appel et le
remplissage des buffers doivent rester à l'intérieur du bloc gardé.

La garantie est **un appel par activation du nœud vivant**, commun à toutes ses
sorties. Un nœud dont aucune sortie n'est utilisée peut être éliminé puisqu'il
est pur. Sans `ondemand`, l'opérateur est évalué au rythme de son domaine,
ordinairement au rythme audio.

`ondemand` accepte aussi des horloges interprétées comme des comptes : la
garantie générale est un appel par activation interne, et non nécessairement
un appel par échantillon audio. Les exemples FFT utilisent uniquement 0 ou 1.

### 3.2 Une entrée audio et une fenêtre de 1024 échantillons

```faust
il = library("interleave.lib");
si = library("signals.lib");

fft1024 = nffunction(
    (float[1024]) fft1024f|fft1024d|fft1024l|fft1024fx(float[1024]),
    <fft.h>, ""
);

process = il.serialize_in(1024)
        : (il.frame_clock(1024), si.bus(1024))
        : ondemand(fft1024);
```

L'historique est construit **hors du domaine `ondemand`**, au rythme audio.
Le premier déclenchement arrive à t=1023 : la fenêtre contient alors
`x[0]…x[1023]`, dans cet ordre. La fonction externe est ensuite appelée une fois
tous les 1024 échantillons.

Avec une fenêtre de 1024 et un hop de 256, seule l'horloge change :

```faust
process = il.serialize_in(1024)
        : (il.frame_clock_hop(1024, 256), si.bus(1024))
        : ondemand(fft1024);
```

Cette horloge déclenche dès t=255. Les premières fenêtres sont complétées par
les zéros de l'historique initial ; elles ne contiennent pas encore 1024
échantillons reçus. Attendre une première fenêtre complète demanderait une
condition supplémentaire sur l'horloge.

**La collecte de la fenêtre ne peut pas être déplacée dans ce `ondemand`.**
Une fonction appelée une fois tous les 256 échantillons et recevant seulement
l'échantillon courant ne peut pas retrouver les 255 échantillons intermédiaires.
Le modèle `process_sample(x)` appelé à chaque tick, avec buffer et compteur
internes, est un autre contrat : un opérateur externe à état.

### 3.3 Retour vers un flux audio

Pour un opérateur de trame `FX : N→N`, la construction existante reste :

```faust
process = il.interleave(1024, FX);
// ou, pour des trames qui se recouvrent :
// process = il.interleave_hop(1024, 256, FX);
```

`FX` peut composer une `nffunction` FFT, un traitement spectral Faust et une
`nffunction` inverse. Une FFT d'analyse seule expose les bins maintenus ; elle
ne doit pas passer directement dans `interleave` si son arité de sortie diffère
de N. Une fenêtre d'analyse/synthèse et sa normalisation restent nécessaires
pour une reconstruction avec recouvrement.

`nffunction` supprime le protocole de transfert externe, mais ne remplace pas
`serialize_out` : le coût de reconstruction du flux est un problème séparé.

## 4. ABI C proposée

### 4.1 Signature homogène : deux buffers

Pour `float[1024]→float[1024]`, le contrat C recommandé est :

```c
void fft1024f(const float* inputs, float* outputs);
void fft1024d(const double* inputs, double* outputs);
```

Il y a **deux paramètres C**, pas 2048. Les tailles sont fixées par le prototype
Faust et connues de la fonction externe. Le symbole sélectionné dépend de la
précision active ; `float` dans le prototype Faust désigne le type réel de
calcul, comme pour `ffunction`, et non systématiquement le `float` C.

Pour une entrée homogène entière, le buffer est de type `int32_t`. Les types
des buffers d'entrée et de sortie peuvent être différents. Une liste entièrement
homogène est transportée dans un tableau contigu, même si plusieurs groupes de
ce type apparaissent dans le prototype.

### 4.2 Signature hétérogène : deux structures

Pour `(float, int, float)→(int, float, float)` en simple précision :

```c
#include <stdint.h>

typedef struct {
    float g0;
    int32_t g1;
    float g2;
} foo_f32_inputs;

typedef struct {
    int32_t g0;
    float g1;
    float g2;
} foo_f32_outputs;

void foo_f32(const foo_f32_inputs* inputs, foo_f32_outputs* outputs);
```

Dans une liste hétérogène, chaque groupe du prototype devient un champ `g0`,
`g1`, etc., dans l'ordre déclaré. Un groupe répété devient un champ tableau :

```c
typedef struct {
    float g0[1024];
    int32_t g1;
} grouped_f32_inputs;

typedef struct {
    int32_t g0;
    float g1[1024];
} grouped_f32_outputs;

void grouped_f32(const grouped_f32_inputs* inputs,
                 grouped_f32_outputs* outputs);
```

En double précision, les champs réels deviennent `double` et le symbole ainsi
que les noms de types correspondent à la variante double. Les structures
gardent leur alignement C naturel ; aucun cast d'un buffer de réels vers une
structure hétérogène n'est admissible.

Le header externe est l'autorité pour les typedefs et les prototypes. Le
compilateur Faust doit émettre les références aux noms convenus, sans redéfinir
ces structures. Une description ou génération auxiliaire de header pourrait
éviter leur saisie manuelle ; c'est un outil complémentaire, pas une nouvelle
syntaxe requise pour le MVP. Une incompatibilité de déclaration doit être
détectable par le compilateur C/C++, sans conversion de pointeurs qui la masque.

### 4.3 Durée de vie et contraintes

Les buffers appartiennent au DSP généré ou à son invocation ; ils ne sont pas
globaux partagés. La fonction :

- lit uniquement les entrées et écrit tous les éléments de sortie ;
- ne conserve pas les pointeurs après son retour ;
- termine synchronement avant que ses sorties soient utilisées ;
- respecte les tailles et n'accède pas hors des buffers ;
- ne suppose aucun alias entre les buffers d'entrée et de sortie ;
- produit les mêmes résultats pour les mêmes entrées dans un même mode de
  calcul, sans dépendance observable à un état caché.

Le prototype seul ne garantit pas un alignement SIMD particulier. Une FFT qui
l'exige doit utiliser un adaptateur ou un contrat d'alignement explicite.
La préparation des plans FFT et les allocations coûteuses ne doivent pas être
faites à chaque activation audio. Leur gestion demande un contrat de cycle de
vie séparé, hors du MVP pur décrit ici.

## 5. Compilation : de la syntaxe au code généré

Le parcours suit le pipeline du projet :

```text
parse → boxes → eval → propagate → normalize → type/interval
      → transform → FIR → backend
```

### 5.1 Parser, boxes et évaluation

Le parser reconnaît le nouveau mot-clé et deux listes de groupes typés. Il
conserve les tailles comme expressions jusqu'à leur évaluation constante.
Un constructeur et un matcher canoniques portent le descripteur de la
primitive : groupes d'entrée/sortie, variantes du nom, header et bibliothèque.

L'évaluation résout les tailles et valide les types, les arités et leurs
débordements. Le descripteur garde les groupes sous une forme compacte :
`float[1024]` ne devient pas une chaîne de 1024 déclarations de type.

### 5.2 Propagation et représentation signal

La propagation construit **un nœud d'appel multi-résultat**, propriétaire de
ses N arguments, et M projections typées de ce même nœud :

```text
ExternalCall(descriptor, arguments, domaine)
 ├─ Projection(0) → type de sortie 0
 ├─ Projection(1) → type de sortie 1
 └─ …
```

Les arguments restent des signaux Faust scalaires. Chaque projection connaît
son indice et son type ; elle n'embarque pas une copie du calcul complet.
L'appel et les données qui définissent son contrat restent co-localisés dans
le nœud, sans table latérale non documentée.

La normalisation doit préserver le lien appel/projections. Un appel présent
dans plusieurs domaines ne doit pas être partagé entre ces domaines. Si une
mutualisation de deux appels purs équivalents est autorisée, elle reste dans
le même domaine et une position d'exécution compatible ; elle ne traverse
jamais une garde.

### 5.3 Types et intervalles

Chaque port possède son propre type. Les règles de conversion aux types des
arguments reprennent celles de `ffunction` ; une incompatibilité non convertible
est rejetée avant l'émission.

Sans contrat supplémentaire sur la fonction externe, les intervalles de sortie
sont conservateurs. Le compilateur ne doit ni inventer une borne FFT, ni
déduire l'absence de NaN ou d'infini. Les dérivées ne sont pas inférées :
FAD/RAD nécessitent une règle dédiée ; en son absence, la différentiation de
cette primitive doit produire un diagnostic explicite.

### 5.4 Ordonnancement et FIR

Le lowering matérialise l'appel comme une opération qui produit plusieurs
valeurs, avec un ordre explicite :

```text
évaluer les arguments
→ écrire le stockage d'entrée
→ appeler la fonction externe
→ rendre disponibles les sorties
```

La lecture d'une projection dépend de l'opération productrice. Un appel C
`void` qui écrit un buffer n'est pas une expression scalaire indépendante
pour chaque sortie. Le FIR doit pouvoir exprimer les stockages typés, leurs
adresses, l'appel et les dépendances de lecture ; les capacités manquantes
doivent être ajoutées explicitement.

Sous `ondemand`, ces opérations sont émises dans la région gardée existante.
Le stockage des sorties maintenues persiste entre les activations et est remis
à zéro par `instanceClear`. Les entrées temporaires n'ont pas à persister
après l'appel. Pour les grandes tailles, le backend privilégie un stockage
par instance réutilisable plutôt que de gros tableaux automatiques sur la pile.

Le FIR peut conserver le caractère pur du nœud logique tout en exprimant les
écritures locales nécessaires à l'ABI. Les passes CSE et d'ordonnancement doivent
respecter ces dépendances ; le nom d'une fonction et ses seuls arguments ne
suffisent pas à représenter son appel C matérialisé.

### 5.5 Schéma de C généré

Le schéma suivant montre un appel 1024→1024 en simple précision. `window`
contient la fenêtre déjà constituée, dans l'ordre chronologique ; `fire` est
l'horloge externe. Ce code illustre la séquence, pas l'émission exacte des
lignes de retard de Faust.

```c
#include <string.h>
#include <fft.h>

typedef struct {
    float foreign_inputs[1024];
    float held_outputs[1024];
} spectral_state;

void spectral_clear(spectral_state* state)
{
    memset(state->held_outputs, 0, sizeof state->held_outputs);
}

void spectral_tick(spectral_state* state, int fire, const float window[1024])
{
    if (fire != 0) {
        for (int i = 0; i < 1024; ++i) {
            state->foreign_inputs[i] = window[i];
        }
        fft1024f(state->foreign_inputs, state->held_outputs);
    }
    /* Les projections lisent state->held_outputs après ce bloc. */
}
```

L'écriture directe dans le stockage maintenu est possible ici parce que le
retour est synchrone et que toutes les sorties sont définies. Si le lowering
utilise un buffer temporaire de sortie, sa copie vers les valeurs maintenues
reste dans la garde. Un header utilisé depuis du C++ doit fournir les gardes
`extern "C"` habituelles pour une bibliothèque C.

### 5.6 Compilation et édition de liens

Après implémentation, la commande Faust utiliserait les options habituelles :

```sh
faust-rs -single -I libraries -lang cpp fft_analysis.dsp -o fft_analysis.cpp
```

La compilation C++ du DSP et de son architecture doit ensuite trouver `fft.h`
et être liée à l'adaptateur ainsi qu'à sa bibliothèque FFT, par exemple FFTW.
Le choix `-double` doit sélectionner la variante double et les buffers `double`.
Le fichier produit par `-lang cpp` est un DSP à intégrer dans une architecture,
pas nécessairement un exécutable autonome.

Ces commandes décrivent l'utilisation visée ; elles ne constituent pas un test
effectué de `nffunction`. Aucun nouveau flag CLI n'est requis pour le MVP.

## 6. Gains attendus et limites

| Élément | Effet attendu |
|---|---|
| Écritures externes chaînées | Supprimées ; le compilateur remplit un stockage d'entrée. |
| Lectures externes séparées | Remplacées par les projections d'un appel commun. |
| Signature C | Deux pointeurs au lieu de N+M paramètres scalaires. |
| Cœur FFT | Appel de bibliothèque ; les papillons ne sont pas dépliés dans le graphe Faust. |
| Transfert d'une trame | O(N+M) par activation, hors calcul externe. |
| Taille du graphe aux frontières | N entrées et M projections restent représentées ; coût au moins O(N+M). |
| Historique audio | Toujours nécessaire à l'extérieur de `ondemand`. |
| Reconstruction `serialize_out` | Inchangée par cette primitive. |

La notation `float[N]` réduit la taille du texte et du descripteur, mais ne
rend pas constant le coût total de compilation. Le compilateur devrait pouvoir
émettre des boucles de copie lorsque les données sont indexables ; il ne peut
pas transformer arbitrairement N expressions scalaires différentes en une
seule lecture de tableau.

Pour supprimer également les ports scalaires aux frontières, il faudrait une
représentation de buffers dans le graphe et des opérateurs de collecte et de
restitution. Pour remplacer la reconstruction par une accumulation circulaire,
le chantier OLA décrit dans le plan de performance reste applicable.

Une fonction qui modifie un état persistant observable, utilise un contexte
par instance ou peut échouer exige une extension distincte : contrat de cycle
de vie, initialisation, remise à zéro, destruction et politique d'erreur.
Ces comportements ne doivent pas être cachés dans le MVP pur de `nffunction`.

## 7. Périmètre d'implémentation proposé

Le MVP proposé couvre `int` et `float`, les groupes constants, plusieurs
sorties, l'ABI à deux pointeurs, les backends C/C++ en simple/double précision,
et les domaines booléens `ondemand`. L'intégration au mode vectoriel doit
préserver le bloc gardé et ses dépendances ; un mode non validé doit être rejeté
explicitement plutôt que produire un code incorrect.

Les autres backends nécessitent une politique explicite de liaison et
d'exécution externe. L'interpréteur ou le JIT ne peuvent pas appeler un symbole
arbitraire seulement parce qu'un header C apparaît dans la source. Les modes
non pris en charge doivent diagnostiquer la primitive.

Avant le code, la syntaxe et l'ABI proposées doivent être validées, ainsi que
le périmètre des backends. L'implémentation suit les frontières de crates :

| Couche | Travail |
|---|---|
| `parser`, `boxes`, `eval` | Syntaxe, constructeur/matcher, descripteur compact, constantes et arités. |
| `propagate`, `signals`, `normalize` | Appel commun, projections et préservation des domaines. |
| `sigtype` | Types par port et intervalles conservateurs. |
| `transform`, `fir` | Ordonnancement, stockages/adresses, appel et maintien des sorties. |
| `codegen` | ABI, headers, variantes de précision et émission C/C++. |
| `compiler` | Diagnostics, intégration et validation de bout en bout. |

## 8. Validation et critères d'acceptation

Les tests doivent d'abord utiliser de petites fonctions externes déterministes,
sans dépendance à une installation locale de Faust ou à une bibliothèque FFT.
Une FFT réelle constitue ensuite un test d'intégration complémentaire.

1. **Signature et arité** : groupes constants, entrées/sorties hétérogènes,
   ordre des ports, tailles invalides et débordements diagnostiqués.
2. **Résultats multiples** : une fonction fournit plusieurs valeurs distinctes ;
   toutes les projections proviennent du même appel. Inspecter le FIR/code
   généré pour vérifier qu'il n'est pas dupliqué par sortie.
3. **Horloge** : aucun appel sur les ticks inactifs ; un appel par déclenchement
   booléen ; sorties nulles initialement et maintenues entre déclenchements.
4. **Isolation** : deux domaines distincts et deux instances DSP ne partagent
   ni buffers ni résultats ; remise à zéro conforme au cycle de vie Faust.
5. **ABI** : compiler et exécuter le C/C++ généré avec le header/adaptateur
   correspondant, en simple et double précision, pour les deux formes de
   stockage. Vérifier les types entiers et l'absence de cast masquant un défaut.
6. **FFT** : comparaison des bins à une DFT de référence sur petites tailles,
   puis analyse/FFT inverse avec les conventions de rangement et de
   normalisation documentées. Vérifier les fenêtres aux déclenchements.
7. **Optimisations** : mêmes résultats en mode non optimisé et optimisé ;
   aucun appel déplacé hors de sa garde. Toute instrumentation de comptage est
   un outil de test, pas un effet autorisé par le contrat pur.
8. **Coût** : mesurer temps de compilation, taille du FIR/code et mémoire pour
   N=256, 512, 1024, 2048 et 4096 ; vérifier la disparition du chaînage et ne pas
   annoncer un coût constant pour les frontières scalaires restantes.

Avant un commit d'implémentation : exécuter les gates de `AGENTS.md`, notamment
le budget de compilation, et actualiser le registre des différences avec la
référence C++. Le mapping public serait une **extension**, pas un port 1:1.
La présente note ne déclare aucune nouvelle syntaxe comme déjà prise en charge.

## 9. Références dans le dépôt

- [Domaines d'horloge : `ondemand`, `upsampling`, `downsampling`](../docs/ondemand-note-fr.md).
- [Définitions de `interleave`, des horloges et de la sérialisation](../libraries/interleave.lib).
- [Sémantique de `interleave` et convention de phase](interleave-spectral-primitive-2026-07-07-en.md).
- [Scalabilité FFT, CSE dans les blocs gardés et coût de la reconstruction](fft-scalability-cse-in-clocked-blocks-2026-07-09-en.md).
- [Tests de la FFT par trame en pur Faust](../crates/compiler/tests/interleave_fft.rs).
- [Décodage et lowering actuels de `ffunction`](../crates/transform/src/signal_fir/module/core_lowering.rs).
- [Registre des différences avec Faust C++](faust-rs-vs-faust-cpp-differences-en.md).
- [Règles du projet](../AGENTS.md).
